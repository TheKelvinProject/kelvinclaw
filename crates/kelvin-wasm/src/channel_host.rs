use std::fmt::Display;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use kelvin_core::{KelvinError, KelvinResult};
use serde_json::Value;
use wasmtime::{Caller, Config, Engine, Linker, Memory, Module, Store};

pub mod channel_abi {
    pub const ABI_VERSION: &str = "1.0.0";
    pub const MODULE: &str = "kelvin_channel_host_v1";

    pub const EXPORT_ALLOC: &str = "alloc";
    pub const EXPORT_DEALLOC: &str = "dealloc";
    pub const EXPORT_HANDLE_INGEST: &str = "handle_ingest";
    pub const EXPORT_MEMORY: &str = "memory";

    pub const IMPORT_LOG: &str = "log";
    pub const IMPORT_CLOCK_NOW_MS: &str = "clock_now_ms";
}

const DEFAULT_MAX_REQUEST_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSandboxPolicy {
    pub max_module_bytes: usize,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub fuel_budget: u64,
}

impl Default for ChannelSandboxPolicy {
    fn default() -> Self {
        Self {
            max_module_bytes: super::DEFAULT_MAX_MODULE_BYTES,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            fuel_budget: super::DEFAULT_FUEL_BUDGET,
        }
    }
}

#[derive(Debug, Clone)]
struct ChannelHostState {
    policy: ChannelSandboxPolicy,
}

#[derive(Clone)]
pub struct WasmChannelHost {
    engine: Engine,
}

impl Default for WasmChannelHost {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmChannelHost {
    pub fn new() -> Self {
        Self::try_new().expect("create wasm channel host engine")
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
        input_json: &str,
        policy: ChannelSandboxPolicy,
    ) -> KelvinResult<String> {
        let wasm_bytes = std::fs::read(wasm_path).map_err(KelvinError::from)?;
        self.run_bytes(&wasm_bytes, input_json, policy)
    }

    pub fn run_bytes(
        &self,
        wasm_bytes: &[u8],
        input_json: &str,
        policy: ChannelSandboxPolicy,
    ) -> KelvinResult<String> {
        if wasm_bytes.len() > policy.max_module_bytes {
            return Err(KelvinError::InvalidInput(format!(
                "wasm channel module size {} exceeds limit {}",
                wasm_bytes.len(),
                policy.max_module_bytes
            )));
        }
        if input_json.len() > policy.max_request_bytes {
            return Err(KelvinError::InvalidInput(format!(
                "channel input exceeds max_request_bytes {}",
                policy.max_request_bytes
            )));
        }

        let module = Module::new(&self.engine, wasm_bytes)
            .map_err(|err| backend("compile channel wasm module", err))?;
        validate_imports(&module)?;

        let mut store = Store::new(
            &self.engine,
            ChannelHostState {
                policy: policy.clone(),
            },
        );
        store
            .set_fuel(policy.fuel_budget)
            .map_err(|err| backend("set channel fuel budget", err))?;

        let mut linker = Linker::<ChannelHostState>::new(&self.engine);
        self.link_channel_imports(&mut linker)?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|err| backend("instantiate channel module", err))?;

        let memory = instance
            .get_memory(&mut store, channel_abi::EXPORT_MEMORY)
            .ok_or_else(|| {
                KelvinError::InvalidInput("channel module must export memory".to_string())
            })?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, channel_abi::EXPORT_ALLOC)
            .map_err(|err| backend("resolve channel alloc export", err))?;
        let dealloc = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, channel_abi::EXPORT_DEALLOC)
            .map_err(|err| backend("resolve channel dealloc export", err))?;
        let handle_ingest = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, channel_abi::EXPORT_HANDLE_INGEST)
            .map_err(|err| backend("resolve channel handle_ingest export", err))?;

        let input_ptr = alloc
            .call(
                &mut store,
                i32::try_from(input_json.len()).map_err(|_| {
                    KelvinError::InvalidInput(
                        "channel input exceeded i32 address space".to_string(),
                    )
                })?,
            )
            .map_err(|err| backend("call channel alloc for input", err))?;
        write_guest_bytes(
            &memory,
            &mut store,
            input_ptr,
            input_json.as_bytes(),
            "write channel input bytes",
        )?;

        let ingest_result = handle_ingest.call(
            &mut store,
            (
                input_ptr,
                i32::try_from(input_json.len()).map_err(|_| {
                    KelvinError::InvalidInput(
                        "channel input exceeded i32 address space".to_string(),
                    )
                })?,
            ),
        );
        let _ = dealloc.call(
            &mut store,
            (
                input_ptr,
                i32::try_from(input_json.len()).unwrap_or_default(),
            ),
        );

        let packed = ingest_result.map_err(|err| {
            if matches!(store.get_fuel(), Ok(0)) {
                KelvinError::Timeout("channel execution exceeded fuel budget".to_string())
            } else {
                backend("execute channel handle_ingest export", err)
            }
        })?;
        let (output_ptr, output_len) = unpack_ptr_len(packed, "channel handle_ingest return")?;
        let output = read_guest_bytes(
            &memory,
            &mut store,
            output_ptr,
            output_len,
            policy.max_response_bytes,
            "read channel output",
        )?;
        let _ = dealloc.call(&mut store, (output_ptr, output_len));
        let text = String::from_utf8(output).map_err(|err| {
            KelvinError::InvalidInput(format!("channel output must be utf-8 json: {err}"))
        })?;
        serde_json::from_str::<Value>(&text).map_err(|err| {
            KelvinError::InvalidInput(format!("channel output must be json: {err}"))
        })?;
        Ok(text)
    }

    fn link_channel_imports(&self, linker: &mut Linker<ChannelHostState>) -> KelvinResult<()> {
        linker
            .func_wrap(
                channel_abi::MODULE,
                channel_abi::IMPORT_LOG,
                |mut caller: Caller<'_, ChannelHostState>,
                 _level: i32,
                 ptr: i32,
                 len: i32|
                 -> i32 {
                    let max_len = caller.data().policy.max_request_bytes.min(16 * 1024);
                    if let Ok(bytes) =
                        read_caller_bytes(&mut caller, ptr, len, max_len, "channel log message")
                    {
                        let _ = String::from_utf8(bytes);
                    }
                    0
                },
            )
            .map_err(|err| backend("link channel log import", err))?;

        linker
            .func_wrap(
                channel_abi::MODULE,
                channel_abi::IMPORT_CLOCK_NOW_MS,
                || -> i64 {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
                        .unwrap_or_default()
                },
            )
            .map_err(|err| backend("link channel clock import", err))?;

        Ok(())
    }
}

fn validate_imports(module: &Module) -> KelvinResult<()> {
    for import in module.imports() {
        if import.module() != channel_abi::MODULE {
            return Err(KelvinError::InvalidInput(format!(
                "unsupported channel import module '{}' for ABI {} (expected '{}')",
                import.module(),
                channel_abi::ABI_VERSION,
                channel_abi::MODULE
            )));
        }
        match import.name() {
            channel_abi::IMPORT_LOG | channel_abi::IMPORT_CLOCK_NOW_MS => {}
            name => {
                return Err(KelvinError::InvalidInput(format!(
                    "unsupported channel ABI {} import '{}.{}'",
                    channel_abi::ABI_VERSION,
                    import.module(),
                    name
                )));
            }
        }
    }
    Ok(())
}

fn read_guest_bytes(
    memory: &Memory,
    store: &mut Store<ChannelHostState>,
    ptr: i32,
    len: i32,
    max_len: usize,
    context: &str,
) -> KelvinResult<Vec<u8>> {
    if ptr < 0 || len < 0 {
        return Err(KelvinError::InvalidInput(format!(
            "{context}: pointer/length must be non-negative"
        )));
    }
    let len = usize::try_from(len)
        .map_err(|_| KelvinError::InvalidInput(format!("{context}: length conversion overflow")))?;
    if len > max_len {
        return Err(KelvinError::InvalidInput(format!(
            "{context}: payload size {} exceeds max {}",
            len, max_len
        )));
    }
    let mut bytes = vec![0_u8; len];
    memory
        .read(store, usize::try_from(ptr).unwrap_or_default(), &mut bytes)
        .map_err(|err| {
            KelvinError::InvalidInput(format!("{context}: memory read failed: {err}"))
        })?;
    Ok(bytes)
}

fn write_guest_bytes(
    memory: &Memory,
    store: &mut Store<ChannelHostState>,
    ptr: i32,
    bytes: &[u8],
    context: &str,
) -> KelvinResult<()> {
    if ptr < 0 {
        return Err(KelvinError::InvalidInput(format!(
            "{context}: pointer must be non-negative"
        )));
    }
    memory
        .write(store, usize::try_from(ptr).unwrap_or_default(), bytes)
        .map_err(|err| KelvinError::InvalidInput(format!("{context}: memory write failed: {err}")))
}

fn read_caller_bytes(
    caller: &mut Caller<'_, ChannelHostState>,
    ptr: i32,
    len: i32,
    max_len: usize,
    context: &str,
) -> KelvinResult<Vec<u8>> {
    if ptr < 0 || len < 0 {
        return Err(KelvinError::InvalidInput(format!(
            "{context}: pointer/length must be non-negative"
        )));
    }
    let len = usize::try_from(len)
        .map_err(|_| KelvinError::InvalidInput(format!("{context}: length conversion overflow")))?;
    if len > max_len {
        return Err(KelvinError::InvalidInput(format!(
            "{context}: payload size {} exceeds max {}",
            len, max_len
        )));
    }

    let memory = caller
        .get_export(channel_abi::EXPORT_MEMORY)
        .and_then(|export| export.into_memory())
        .ok_or_else(|| KelvinError::InvalidInput("channel memory export missing".to_string()))?;
    let mut bytes = vec![0_u8; len];
    memory
        .read(caller, usize::try_from(ptr).unwrap_or_default(), &mut bytes)
        .map_err(|err| {
            KelvinError::InvalidInput(format!("{context}: memory read failed: {err}"))
        })?;
    Ok(bytes)
}

fn unpack_ptr_len(value: i64, context: &str) -> KelvinResult<(i32, i32)> {
    if value <= 0 {
        return Err(KelvinError::Backend(format!(
            "{context}: packed pointer/length is invalid"
        )));
    }
    let raw = value as u64;
    let ptr = (raw >> 32) as u32;
    let len = (raw & 0xFFFF_FFFF) as u32;
    if len == 0 {
        return Ok((ptr as i32, 0));
    }
    if ptr == 0 {
        return Err(KelvinError::Backend(format!(
            "{context}: non-empty payload has null pointer"
        )));
    }
    Ok((ptr as i32, len as i32))
}

fn backend(context: &str, err: impl Display) -> KelvinError {
    KelvinError::Backend(format!("{context}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::{channel_abi, ChannelSandboxPolicy, WasmChannelHost};
    use kelvin_core::KelvinError;

    fn parse_wat(input: &str) -> Vec<u8> {
        wat::parse_str(input).expect("parse wat")
    }

    fn test_module() -> Vec<u8> {
        parse_wat(
            r#"
            (module
              (import "kelvin_channel_host_v1" "log" (func $log (param i32 i32 i32) (result i32)))
              (import "kelvin_channel_host_v1" "clock_now_ms" (func $clock_now_ms (result i64)))
              (memory (export "memory") 2)
              (global $heap (mut i32) (i32.const 1024))
              (func (export "alloc") (param $len i32) (result i32)
                (local $ptr i32)
                global.get $heap
                local.tee $ptr
                local.get $len
                i32.add
                global.set $heap
                local.get $ptr)
              (func (export "dealloc") (param i32 i32))
              (data (i32.const 2048) "{\"allow\":true,\"reason\":\"ok\"}")
              (func (export "handle_ingest") (param i32 i32) (result i64)
                i32.const 1
                i32.const 2048
                i32.const 6
                call $log
                drop
                i32.const 2048
                i64.extend_i32_u
                i64.const 32
                i64.shl
                i32.const 28
                i64.extend_i32_u
                i64.or)
            )
            "#,
        )
    }

    #[test]
    fn channel_host_roundtrip_returns_json_payload() {
        let host = WasmChannelHost::try_new().expect("create channel host");
        let output = host
            .run_bytes(
                &test_module(),
                r#"{"channel":"telegram","sender_id":"7"}"#,
                ChannelSandboxPolicy::default(),
            )
            .expect("run channel module");
        assert_eq!(output, r#"{"allow":true,"reason":"ok"}"#);
    }

    #[test]
    fn channel_host_rejects_unsupported_import_module() {
        let wasm = parse_wat(
            r#"
            (module
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (memory (export "memory") 1)
              (func (export "alloc") (param i32) (result i32) i32.const 0)
              (func (export "dealloc") (param i32 i32))
              (func (export "handle_ingest") (param i32 i32) (result i64) i64.const 0)
            )
            "#,
        );
        let host = WasmChannelHost::try_new().expect("host");
        let err = host
            .run_bytes(&wasm, "{}", ChannelSandboxPolicy::default())
            .expect_err("unsupported import should fail");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
        assert!(err
            .to_string()
            .contains("unsupported channel import module"));
    }

    #[test]
    fn channel_host_enforces_request_bounds() {
        let host = WasmChannelHost::try_new().expect("host");
        let policy = ChannelSandboxPolicy {
            max_request_bytes: 8,
            ..ChannelSandboxPolicy::default()
        };
        let err = host
            .run_bytes(&test_module(), "{\"too\":\"long\"}", policy)
            .expect_err("request bound should fail");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
        assert!(err.to_string().contains("max_request_bytes"));
    }

    #[test]
    fn abi_constants_are_stable() {
        assert_eq!(channel_abi::MODULE, "kelvin_channel_host_v1");
        assert_eq!(channel_abi::EXPORT_HANDLE_INGEST, "handle_ingest");
    }
}
