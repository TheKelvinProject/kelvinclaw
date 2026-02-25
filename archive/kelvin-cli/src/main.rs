use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use kelvin_brain::{load_installed_tool_plugins_default, EchoModelProvider, KelvinBrain};
use kelvin_core::{
    now_ms, AgentRunRequest, KelvinError, KelvinResult, PluginSecurityPolicy, ToolRegistry,
};
use kelvin_memory::{MemoryBackendKind, MemoryFactory};
use kelvin_runtime::{
    AgentRuntime, HashMapToolRegistry, InMemorySessionStore, LaneScheduler, RunOutcome,
    RunRegistry, StaticTextTool, StdoutEventSink, TimeTool,
};

#[derive(Debug, Clone)]
struct CliConfig {
    prompt: String,
    session_id: String,
    workspace_dir: PathBuf,
    memory_backend: MemoryBackendKind,
    timeout_ms: u64,
    system_prompt: Option<String>,
}

fn usage() -> &'static str {
    "Usage: kelvin-cli --prompt <text> [--session <id>] [--workspace <dir>] [--memory markdown|in-memory|fallback] [--timeout-ms <ms>]"
}

struct CombinedToolRegistry {
    installed: Arc<dyn ToolRegistry>,
    builtins: Arc<dyn ToolRegistry>,
}

impl CombinedToolRegistry {
    fn new(installed: Arc<dyn ToolRegistry>, builtins: Arc<dyn ToolRegistry>) -> Self {
        Self { installed, builtins }
    }
}

impl ToolRegistry for CombinedToolRegistry {
    fn get(&self, name: &str) -> Option<Arc<dyn kelvin_core::Tool>> {
        self.installed
            .get(name)
            .or_else(|| self.builtins.get(name))
    }

    fn names(&self) -> Vec<String> {
        let mut names = self.installed.names();
        names.extend(self.builtins.names());
        names.sort();
        names.dedup();
        names
    }
}

fn parse_args() -> Result<CliConfig, String> {
    let mut prompt: Option<String> = None;
    let mut session_id = "main".to_string();
    let mut workspace_dir = env::current_dir().map_err(|err| err.to_string())?;
    let mut memory_backend = MemoryBackendKind::Markdown;
    let mut timeout_ms = 30_000_u64;
    let mut system_prompt: Option<String> = None;

    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(usage().to_string()),
            "--prompt" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --prompt".to_string())?;
                prompt = Some(value);
            }
            "--session" => {
                session_id = args
                    .next()
                    .ok_or_else(|| "missing value for --session".to_string())?;
            }
            "--workspace" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --workspace".to_string())?;
                workspace_dir = PathBuf::from(value);
            }
            "--memory" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --memory".to_string())?;
                memory_backend = MemoryBackendKind::parse(&value);
            }
            "--timeout-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --timeout-ms".to_string())?;
                timeout_ms = value
                    .parse::<u64>()
                    .map_err(|_| "invalid numeric value for --timeout-ms".to_string())?;
            }
            "--system" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --system".to_string())?;
                system_prompt = Some(value);
            }
            other if !other.starts_with('-') && prompt.is_none() => {
                prompt = Some(other.to_string());
            }
            unknown => {
                return Err(format!("unknown argument: {unknown}\n{}", usage()));
            }
        }
    }

    let prompt = prompt.ok_or_else(|| format!("missing prompt\n{}", usage()))?;

    Ok(CliConfig {
        prompt,
        session_id,
        workspace_dir,
        memory_backend,
        timeout_ms,
        system_prompt,
    })
}

async fn run(config: CliConfig) -> KelvinResult<()> {
    let session_store = Arc::new(InMemorySessionStore::default());
    let event_sink = Arc::new(StdoutEventSink);

    let builtin_tools = Arc::new(HashMapToolRegistry::new());
    builtin_tools.register(TimeTool);
    builtin_tools.register(StaticTextTool::new(
        "hello_tool",
        "Hello from a pluggable tool implementation.",
    ));
    let loaded = load_installed_tool_plugins_default("0.1.0", PluginSecurityPolicy::default())?;
    println!(
        "loaded installed plugins: {}",
        loaded.loaded_plugins.len()
    );

    let tools: Arc<dyn ToolRegistry> = Arc::new(CombinedToolRegistry::new(
        loaded.tool_registry,
        builtin_tools,
    ));

    let memory = MemoryFactory::build(&config.workspace_dir, config.memory_backend);
    let model = Arc::new(EchoModelProvider::new("kelvin", "echo-v1"));
    let brain = Arc::new(KelvinBrain::new(
        session_store,
        memory,
        model,
        tools,
        event_sink,
    ));

    let runtime = AgentRuntime::new(
        brain,
        Arc::new(LaneScheduler::default()),
        Arc::new(RunRegistry::default()),
    );

    let run_id = format!("run-{}", now_ms());
    let request = AgentRunRequest {
        run_id: run_id.clone(),
        session_id: config.session_id.clone(),
        session_key: config.session_id,
        workspace_dir: config.workspace_dir.to_string_lossy().to_string(),
        prompt: config.prompt,
        extra_system_prompt: config.system_prompt,
        timeout_ms: Some(config.timeout_ms),
        memory_query: None,
    };

    let accepted = runtime.submit(request).await?;
    println!("accepted run: {} at {}", accepted.run_id, accepted.accepted_at_ms);

    match runtime
        .wait_for_outcome(&run_id, config.timeout_ms.saturating_add(5_000))
        .await?
    {
        RunOutcome::Completed(result) => {
            println!(
                "run complete in {}ms (provider={}, model={})",
                result.meta.duration_ms, result.meta.provider, result.meta.model
            );
            for payload in result.payloads {
                println!("payload: {}", payload.text);
            }
        }
        RunOutcome::Failed(error) => {
            return Err(KelvinError::Backend(format!("run failed: {error}")));
        }
        RunOutcome::Timeout => {
            return Err(KelvinError::Timeout(
                "timed out waiting for run result".to_string(),
            ));
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    match parse_args() {
        Ok(config) => {
            if let Err(err) = run(config).await {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("{err}");
            if err.starts_with("Usage:") {
                std::process::exit(0);
            }
            std::process::exit(1);
        }
    }
}
