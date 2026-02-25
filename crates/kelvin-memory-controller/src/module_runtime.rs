use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time;
use wasmparser::{Parser, Payload};
use wasmtime::{Caller, Config, Engine, Linker, Module, Store};

use kelvin_core::{KelvinError, KelvinResult};
use kelvin_memory_api::MemoryModuleManifest;
use kelvin_memory_module_sdk::{
    ModuleOperation, EXPORT_HANDLE_DELETE, EXPORT_HANDLE_HEALTH, EXPORT_HANDLE_QUERY,
    EXPORT_HANDLE_READ, EXPORT_HANDLE_UPSERT, HOST_FN_BLOB_GET, HOST_FN_BLOB_PUT,
    HOST_FN_CLOCK_NOW_MS, HOST_FN_EMIT_METRIC, HOST_FN_KV_GET, HOST_FN_KV_PUT, HOST_FN_LOG,
    MEMORY_HOST_IMPORT_MODULE,
};

#[derive(Debug, Clone)]
pub struct ModuleRuntimeConfig {
    pub max_module_bytes: usize,
    pub max_memory_pages: u32,
    pub default_fuel: u64,
    pub default_timeout_ms: u64,
}

#[derive(Default)]
struct HostState;

#[derive(Clone)]
pub struct LoadedMemoryModule {
    manifest: MemoryModuleManifest,
    engine: Engine,
    module: Module,
    config: ModuleRuntimeConfig,
}

impl LoadedMemoryModule {
    pub fn new(
        manifest: MemoryModuleManifest,
        wasm_bytes: &[u8],
        config: ModuleRuntimeConfig,
    ) -> KelvinResult<Self> {
        manifest
            .validate()
            .map_err(|err| KelvinError::InvalidInput(err.to_string()))?;
        if wasm_bytes.len() > config.max_module_bytes {
            return Err(KelvinError::InvalidInput(format!(
                "module '{}' exceeds max bytes {}",
                manifest.module_id, config.max_module_bytes
            )));
        }
        validate_memory_pages(wasm_bytes, config.max_memory_pages)?;

        let mut wasmtime_cfg = Config::new();
        wasmtime_cfg.consume_fuel(true);
        let engine = Engine::new(&wasmtime_cfg)
            .map_err(|err| KelvinError::Backend(format!("engine init failed: {err}")))?;
        let module = Module::new(&engine, wasm_bytes)
            .map_err(|err| KelvinError::InvalidInput(format!("invalid wasm module: {err}")))?;

        for export in [
            EXPORT_HANDLE_UPSERT,
            EXPORT_HANDLE_QUERY,
            EXPORT_HANDLE_READ,
            EXPORT_HANDLE_DELETE,
            EXPORT_HANDLE_HEALTH,
        ] {
            if module.get_export(export).is_none() {
                return Err(KelvinError::InvalidInput(format!(
                    "module '{}' missing required export '{}'",
                    manifest.module_id, export
                )));
            }
        }

        Ok(Self {
            manifest,
            engine,
            module,
            config,
        })
    }

    pub fn manifest(&self) -> &MemoryModuleManifest {
        &self.manifest
    }

    pub async fn execute(
        &self,
        operation: ModuleOperation,
        timeout_ms: Option<u64>,
        fuel: Option<u64>,
    ) -> KelvinResult<()> {
        let engine = self.engine.clone();
        let module = self.module.clone();
        let export_name = operation.export_name().to_string();
        let fuel = fuel.unwrap_or(self.config.default_fuel);
        let timeout_ms = timeout_ms.unwrap_or(self.config.default_timeout_ms);

        let mut task = tokio::task::spawn_blocking(move || -> KelvinResult<()> {
            let mut store = Store::new(&engine, HostState);
            store
                .set_fuel(fuel)
                .map_err(|err| KelvinError::Backend(format!("failed to set fuel: {err}")))?;

            let mut linker = Linker::new(&engine);
            linker
                .func_wrap(
                    MEMORY_HOST_IMPORT_MODULE,
                    HOST_FN_KV_GET,
                    |_caller: Caller<'_, HostState>, _handle: i32| -> i32 { -1 },
                )
                .map_err(|err| KelvinError::Backend(format!("link kv_get failed: {err}")))?;
            linker
                .func_wrap(
                    MEMORY_HOST_IMPORT_MODULE,
                    HOST_FN_KV_PUT,
                    |_caller: Caller<'_, HostState>, _handle: i32| -> i32 { 0 },
                )
                .map_err(|err| KelvinError::Backend(format!("link kv_put failed: {err}")))?;
            linker
                .func_wrap(
                    MEMORY_HOST_IMPORT_MODULE,
                    HOST_FN_BLOB_GET,
                    |_caller: Caller<'_, HostState>, _handle: i32| -> i32 { -1 },
                )
                .map_err(|err| KelvinError::Backend(format!("link blob_get failed: {err}")))?;
            linker
                .func_wrap(
                    MEMORY_HOST_IMPORT_MODULE,
                    HOST_FN_BLOB_PUT,
                    |_caller: Caller<'_, HostState>, _handle: i32| -> i32 { 0 },
                )
                .map_err(|err| KelvinError::Backend(format!("link blob_put failed: {err}")))?;
            linker
                .func_wrap(
                    MEMORY_HOST_IMPORT_MODULE,
                    HOST_FN_EMIT_METRIC,
                    |_caller: Caller<'_, HostState>, _handle: i32| -> i32 { 0 },
                )
                .map_err(|err| KelvinError::Backend(format!("link emit_metric failed: {err}")))?;
            linker
                .func_wrap(
                    MEMORY_HOST_IMPORT_MODULE,
                    HOST_FN_LOG,
                    |_caller: Caller<'_, HostState>, _handle: i32| -> i32 { 0 },
                )
                .map_err(|err| KelvinError::Backend(format!("link log failed: {err}")))?;
            linker
                .func_wrap(
                    MEMORY_HOST_IMPORT_MODULE,
                    HOST_FN_CLOCK_NOW_MS,
                    |_caller: Caller<'_, HostState>| -> i64 {
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|value| value.as_millis() as i64)
                            .unwrap_or_default()
                    },
                )
                .map_err(|err| KelvinError::Backend(format!("link clock_now_ms failed: {err}")))?;

            let instance = linker
                .instantiate(&mut store, &module)
                .map_err(|err| KelvinError::Backend(format!("instantiate failed: {err}")))?;
            let func = instance
                .get_typed_func::<(), i32>(&mut store, &export_name)
                .map_err(|err| KelvinError::Backend(format!("missing typed export: {err}")))?;
            let code = func
                .call(&mut store, ())
                .map_err(|err| KelvinError::Backend(format!("module execution trap: {err}")))?;
            if code != 0 {
                return Err(KelvinError::Backend(format!(
                    "module '{}' returned non-zero code {} for op '{}'",
                    module.name().unwrap_or("unknown"),
                    code,
                    export_name
                )));
            }
            Ok(())
        });

        match time::timeout(Duration::from_millis(timeout_ms), &mut task).await {
            Ok(result) => result
                .map_err(|err| KelvinError::Backend(format!("module task join failed: {err}")))?,
            Err(_) => {
                task.abort();
                Err(KelvinError::Timeout(format!(
                    "module execution timed out after {timeout_ms}ms"
                )))
            }
        }
    }
}

fn validate_memory_pages(bytes: &[u8], max_memory_pages: u32) -> KelvinResult<()> {
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::MemorySection(section) =
            payload.map_err(|err| KelvinError::InvalidInput(err.to_string()))?
        {
            for memory in section {
                let memory = memory.map_err(|err| KelvinError::InvalidInput(err.to_string()))?;
                if memory.initial > u64::from(max_memory_pages) {
                    return Err(KelvinError::InvalidInput(format!(
                        "module initial memory pages {} exceed limit {}",
                        memory.initial, max_memory_pages
                    )));
                }
                if let Some(maximum) = memory.maximum {
                    if maximum > u64::from(max_memory_pages) {
                        return Err(KelvinError::InvalidInput(format!(
                            "module max memory pages {} exceed limit {}",
                            maximum, max_memory_pages
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}
