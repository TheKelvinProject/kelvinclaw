use std::fmt::Display;
use std::path::Path;

use kelvin_core::{KelvinError, KelvinResult};
use wasmtime::{Caller, Config, Engine, Linker, Module, Store};

pub mod model_host;
pub use model_host::{
    model_abi, EnvOpenAiResponsesTransport, ModelSandboxPolicy, OpenAiResponsesTransport,
    WasmModelHost,
};

pub mod channel_host;
pub use channel_host::{channel_abi, ChannelSandboxPolicy, WasmChannelHost};

pub mod claw_abi {
    pub const ABI_VERSION: &str = "1.0.0";
    pub const MODULE: &str = "claw";
    pub const RUN_EXPORT: &str = "run";
    pub const SEND_MESSAGE: &str = "send_message";
    pub const MOVE_SERVO: &str = "move_servo";
    pub const FS_READ: &str = "fs_read";
    pub const NETWORK_SEND: &str = "network_send";
}

pub const DEFAULT_MAX_MODULE_BYTES: usize = 512 * 1024;
pub const DEFAULT_FUEL_BUDGET: u64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClawCall {
    SendMessage { message_code: i32 },
    MoveServo { channel: i32, position: i32 },
    FsRead { handle: i32 },
    NetworkSend { packet: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPreset {
    LockedDown,
    DevLocal,
    HardwareControl,
}

impl SandboxPreset {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_lowercase().as_str() {
            "locked_down" | "locked-down" | "locked" => Some(Self::LockedDown),
            "dev_local" | "dev-local" | "dev" => Some(Self::DevLocal),
            "hardware_control" | "hardware-control" | "hardware" => Some(Self::HardwareControl),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::LockedDown => "locked_down",
            Self::DevLocal => "dev_local",
            Self::HardwareControl => "hardware_control",
        }
    }

    pub fn policy(self) -> SandboxPolicy {
        match self {
            Self::LockedDown => SandboxPolicy::locked_down(),
            Self::DevLocal => SandboxPolicy {
                allow_fs_read: true,
                ..SandboxPolicy::locked_down()
            },
            Self::HardwareControl => SandboxPolicy {
                allow_move_servo: true,
                ..SandboxPolicy::locked_down()
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub allow_move_servo: bool,
    pub allow_fs_read: bool,
    pub allow_network_send: bool,
    pub max_module_bytes: usize,
    pub fuel_budget: u64,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            allow_move_servo: false,
            allow_fs_read: false,
            allow_network_send: false,
            max_module_bytes: DEFAULT_MAX_MODULE_BYTES,
            fuel_budget: DEFAULT_FUEL_BUDGET,
        }
    }
}

impl SandboxPolicy {
    pub fn locked_down() -> Self {
        Self::default()
    }

    pub fn allow_all() -> Self {
        Self {
            allow_move_servo: true,
            allow_fs_read: true,
            allow_network_send: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillExecution {
    pub exit_code: i32,
    pub calls: Vec<ClawCall>,
}

#[derive(Debug, Default)]
struct HostState {
    calls: Vec<ClawCall>,
}

impl HostState {
    fn record(&mut self, call: ClawCall) {
        self.calls.push(call);
    }
}

#[derive(Clone)]
pub struct WasmSkillHost {
    engine: Engine,
}

impl Default for WasmSkillHost {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmSkillHost {
    pub fn new() -> Self {
        Self::try_new().expect("create wasm skill host engine")
    }

    pub fn try_new() -> KelvinResult<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|err| backend("create engine", err))?;
        Ok(Self { engine })
    }

    pub fn run_file(
        &self,
        wasm_path: impl AsRef<Path>,
        policy: SandboxPolicy,
    ) -> KelvinResult<SkillExecution> {
        let bytes = std::fs::read(wasm_path).map_err(KelvinError::from)?;
        self.run_bytes(&bytes, policy)
    }

    pub fn run_bytes(
        &self,
        wasm_bytes: &[u8],
        policy: SandboxPolicy,
    ) -> KelvinResult<SkillExecution> {
        if wasm_bytes.len() > policy.max_module_bytes {
            return Err(KelvinError::InvalidInput(format!(
                "wasm module size {} exceeds limit {}",
                wasm_bytes.len(),
                policy.max_module_bytes
            )));
        }

        let module = Module::new(&self.engine, wasm_bytes)
            .map_err(|err| backend("compile wasm module", err))?;
        validate_imports(&module, policy)?;

        let mut store = Store::new(&self.engine, HostState::default());
        store
            .set_fuel(policy.fuel_budget)
            .map_err(|err| backend("set fuel budget", err))?;

        let mut linker = Linker::<HostState>::new(&self.engine);

        linker
            .func_wrap(
                claw_abi::MODULE,
                claw_abi::SEND_MESSAGE,
                |mut caller: Caller<'_, HostState>, message_code: i32| -> i32 {
                    caller
                        .data_mut()
                        .record(ClawCall::SendMessage { message_code });
                    0
                },
            )
            .map_err(|err| backend("link claw.send_message", err))?;

        if policy.allow_move_servo {
            linker
                .func_wrap(
                    claw_abi::MODULE,
                    claw_abi::MOVE_SERVO,
                    |mut caller: Caller<'_, HostState>, channel: i32, position: i32| -> i32 {
                        caller
                            .data_mut()
                            .record(ClawCall::MoveServo { channel, position });
                        0
                    },
                )
                .map_err(|err| backend("link claw.move_servo", err))?;
        }

        if policy.allow_fs_read {
            linker
                .func_wrap(
                    claw_abi::MODULE,
                    claw_abi::FS_READ,
                    |mut caller: Caller<'_, HostState>, handle: i32| -> i32 {
                        caller.data_mut().record(ClawCall::FsRead { handle });
                        0
                    },
                )
                .map_err(|err| backend("link claw.fs_read", err))?;
        }

        if policy.allow_network_send {
            linker
                .func_wrap(
                    claw_abi::MODULE,
                    claw_abi::NETWORK_SEND,
                    |mut caller: Caller<'_, HostState>, packet: i32| -> i32 {
                        caller.data_mut().record(ClawCall::NetworkSend { packet });
                        0
                    },
                )
                .map_err(|err| backend("link claw.network_send", err))?;
        }

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|err| backend("instantiate module", err))?;
        let run = instance
            .get_typed_func::<(), i32>(&mut store, claw_abi::RUN_EXPORT)
            .map_err(|err| backend("resolve run export", err))?;
        let exit_code = match run.call(&mut store, ()) {
            Ok(code) => code,
            Err(err) => {
                let remaining_fuel = store.get_fuel().ok();
                if matches!(remaining_fuel, Some(0)) {
                    return Err(KelvinError::Timeout(
                        "skill execution exceeded fuel budget".to_string(),
                    ));
                }
                return Err(backend("execute run export", err));
            }
        };

        Ok(SkillExecution {
            exit_code,
            calls: store.data().calls.clone(),
        })
    }
}

fn validate_imports(module: &Module, policy: SandboxPolicy) -> KelvinResult<()> {
    for import in module.imports() {
        if import.module() != claw_abi::MODULE {
            return Err(KelvinError::InvalidInput(format!(
                "unsupported import module '{}' for ABI {} (expected '{}')",
                import.module(),
                claw_abi::ABI_VERSION,
                claw_abi::MODULE
            )));
        }

        let name = import.name();
        match name {
            claw_abi::SEND_MESSAGE => {}
            claw_abi::MOVE_SERVO if policy.allow_move_servo => {}
            claw_abi::FS_READ if policy.allow_fs_read => {}
            claw_abi::NETWORK_SEND if policy.allow_network_send => {}
            claw_abi::MOVE_SERVO | claw_abi::FS_READ | claw_abi::NETWORK_SEND => {
                return Err(KelvinError::InvalidInput(format!(
                    "capability import '{name}' denied by sandbox policy"
                )));
            }
            _ => {
                return Err(KelvinError::InvalidInput(format!(
                    "unsupported ABI {} import '{}.{}'",
                    claw_abi::ABI_VERSION,
                    import.module(),
                    name
                )));
            }
        }
    }
    Ok(())
}

fn backend(context: &str, err: impl Display) -> KelvinError {
    KelvinError::Backend(format!("{context}: {err}"))
}

#[cfg(test)]
mod tests {
    use kelvin_core::KelvinError;

    use super::{ClawCall, SandboxPolicy, SandboxPreset, WasmSkillHost};

    fn parse_wat(input: &str) -> Vec<u8> {
        wat::parse_str(input).expect("parse wat")
    }

    #[test]
    fn preset_policies_match_expected_capabilities() {
        assert_eq!(
            SandboxPreset::LockedDown.policy(),
            SandboxPolicy::locked_down()
        );
        assert!(SandboxPreset::DevLocal.policy().allow_fs_read);
        assert!(!SandboxPreset::DevLocal.policy().allow_network_send);
        assert!(SandboxPreset::HardwareControl.policy().allow_move_servo);
        assert!(!SandboxPreset::HardwareControl.policy().allow_fs_read);
    }

    #[test]
    fn runs_skill_with_allowed_claw_call() {
        let wasm = parse_wat(
            r#"
            (module
              (import "claw" "send_message" (func $send_message (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 7
                call $send_message
                drop
                i32.const 0
              )
            )
            "#,
        );

        let host = WasmSkillHost::try_new().expect("host");
        let result = host
            .run_bytes(&wasm, SandboxPolicy::locked_down())
            .expect("run allowed skill");
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.calls,
            vec![ClawCall::SendMessage { message_code: 7 }]
        );
    }

    #[test]
    fn rejects_skill_when_policy_blocks_fs_call() {
        let wasm = parse_wat(
            r#"
            (module
              (import "claw" "fs_read" (func $fs_read (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 1
                call $fs_read
              )
            )
            "#,
        );

        let host = WasmSkillHost::try_new().expect("host");
        let err = host
            .run_bytes(&wasm, SandboxPolicy::locked_down())
            .expect_err("policy should reject fs import");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
        assert!(err.to_string().contains("denied by sandbox policy"));
    }

    #[test]
    fn allows_skill_when_policy_enables_fs_call() {
        let wasm = parse_wat(
            r#"
            (module
              (import "claw" "fs_read" (func $fs_read (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 9
                call $fs_read
              )
            )
            "#,
        );

        let host = WasmSkillHost::try_new().expect("host");
        let result = host
            .run_bytes(
                &wasm,
                SandboxPolicy {
                    allow_fs_read: true,
                    ..SandboxPolicy::locked_down()
                },
            )
            .expect("run allowed fs skill");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.calls, vec![ClawCall::FsRead { handle: 9 }]);
    }

    #[test]
    fn rejects_skill_that_requests_wasi_imports() {
        let wasm = parse_wat(
            r#"
            (module
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 0
              )
            )
            "#,
        );

        let host = WasmSkillHost::try_new().expect("host");
        let err = host
            .run_bytes(&wasm, SandboxPolicy::allow_all())
            .expect_err("wasi import should be blocked");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
        assert!(err.to_string().contains("unsupported import module"));
    }

    #[test]
    fn rejects_unknown_abi_import() {
        let wasm = parse_wat(
            r#"
            (module
              (import "claw" "exfiltrate" (func $exfiltrate (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 0
                call $exfiltrate
              )
            )
            "#,
        );

        let host = WasmSkillHost::try_new().expect("host");
        let err = host
            .run_bytes(&wasm, SandboxPolicy::allow_all())
            .expect_err("unknown import should be rejected");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
        assert!(err.to_string().contains("unsupported ABI"));
    }

    #[test]
    fn rejects_oversized_module_before_compile() {
        let host = WasmSkillHost::try_new().expect("host");
        let policy = SandboxPolicy {
            max_module_bytes: 8,
            ..SandboxPolicy::locked_down()
        };
        let err = host
            .run_bytes(&[0_u8; 9], policy)
            .expect_err("oversized module should fail");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
        assert!(err.to_string().contains("exceeds limit"));
    }

    #[test]
    fn times_out_on_fuel_exhaustion() {
        let wasm = parse_wat(
            r#"
            (module
              (func (export "run") (result i32)
                (loop
                  br 0
                )
                i32.const 0
              )
            )
            "#,
        );

        let host = WasmSkillHost::try_new().expect("host");
        let err = host
            .run_bytes(
                &wasm,
                SandboxPolicy {
                    fuel_budget: 500,
                    ..SandboxPolicy::locked_down()
                },
            )
            .expect_err("fuel exhaustion expected");
        assert!(matches!(err, KelvinError::Timeout(_)));
    }
}
