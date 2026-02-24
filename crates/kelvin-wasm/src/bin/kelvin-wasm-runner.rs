use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use kelvin_wasm::{claw_abi, SandboxPreset, WasmSkillHost};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(err) => {
            eprintln!("kelvin-wasm-runner error: {err}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<i32, String> {
    let mut wasm_path: Option<PathBuf> = None;
    let mut preset = SandboxPreset::LockedDown;
    let mut allow_move_servo: Option<bool> = None;
    let mut allow_fs_read: Option<bool> = None;
    let mut allow_network_send: Option<bool> = None;
    let mut max_module_bytes: Option<usize> = None;
    let mut fuel_budget: Option<u64> = None;

    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut idx = 0usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "--help" | "-h" => {
                print_usage();
                return Ok(0);
            }
            "--wasm" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| "missing value for --wasm".to_string())?;
                wasm_path = Some(PathBuf::from(value));
                idx += 2;
            }
            "--policy-preset" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| "missing value for --policy-preset".to_string())?;
                preset = SandboxPreset::parse(value)
                    .ok_or_else(|| format!("unknown policy preset '{value}'"))?;
                idx += 2;
            }
            "--allow-move-servo" => {
                allow_move_servo = Some(true);
                idx += 1;
            }
            "--allow-fs-read" => {
                allow_fs_read = Some(true);
                idx += 1;
            }
            "--allow-network-send" => {
                allow_network_send = Some(true);
                idx += 1;
            }
            "--max-module-bytes" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| "missing value for --max-module-bytes".to_string())?;
                max_module_bytes =
                    Some(value.parse::<usize>().map_err(|err| {
                        format!("invalid --max-module-bytes value '{value}': {err}")
                    })?);
                idx += 2;
            }
            "--fuel-budget" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| "missing value for --fuel-budget".to_string())?;
                fuel_budget = Some(
                    value
                        .parse::<u64>()
                        .map_err(|err| format!("invalid --fuel-budget value '{value}': {err}"))?,
                );
                idx += 2;
            }
            unknown => {
                return Err(format!("unknown argument '{unknown}'"));
            }
        }
    }

    let wasm_path = wasm_path.ok_or_else(|| "missing required --wasm <path>".to_string())?;
    let mut policy = preset.policy();
    if let Some(value) = allow_move_servo {
        policy.allow_move_servo = value;
    }
    if let Some(value) = allow_fs_read {
        policy.allow_fs_read = value;
    }
    if let Some(value) = allow_network_send {
        policy.allow_network_send = value;
    }
    if let Some(value) = max_module_bytes {
        policy.max_module_bytes = value;
    }
    if let Some(value) = fuel_budget {
        policy.fuel_budget = value;
    }

    let host = WasmSkillHost::try_new().map_err(|err| err.to_string())?;
    let execution = host
        .run_file(&wasm_path, policy)
        .map_err(|err| format!("{err}"))?;

    println!("kelvin_abi_version={}", claw_abi::ABI_VERSION);
    println!("policy_preset={}", preset.name());
    println!("exit_code={}", execution.exit_code);
    println!("calls={}", execution.calls.len());
    for call in execution.calls {
        println!("call={call:?}");
    }

    Ok(execution.exit_code)
}

fn print_usage() {
    println!("Usage:");
    println!("  kelvin-wasm-runner --wasm <path> [options]");
    println!();
    println!("Options:");
    println!("  --policy-preset <locked_down|dev_local|hardware_control>  (default: locked_down)");
    println!("  --allow-move-servo");
    println!("  --allow-fs-read");
    println!("  --allow-network-send");
    println!("  --max-module-bytes <usize>");
    println!("  --fuel-budget <u64>");
}
