use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_OUTPUT_BYTES: u64 = 256 * 1024;
const MAX_ERROR_CHARS: usize = 500;

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub prompt: String,
    pub model: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutput {
    pub result: String,
    pub cost_usd: f64,
    pub turns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    #[error("agent indisponible: {0}")]
    Unavailable(String),
    #[error("délai de l'agent dépassé")]
    Timeout,
    #[error("échec de l'agent: {0}")]
    Failed(String),
}

pub type AgentFuture<'a> =
    Pin<Box<dyn Future<Output = Result<AgentOutput, AgentError>> + Send + 'a>>;

pub trait AgentRunner: Send + Sync {
    fn run<'a>(&'a self, request: &'a AgentRequest) -> AgentFuture<'a>;
}

// Exécute `claude -p` sans outil, dans un dossier jetable et un environnement épuré.
#[derive(Clone)]
pub struct ClaudeCli {
    command: PathBuf,
    oauth_token: Option<String>,
}

impl std::fmt::Debug for ClaudeCli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCli")
            .field("command", &self.command)
            .field("oauth_token", &self.oauth_token.as_ref().map(|_| "***"))
            .finish()
    }
}

impl ClaudeCli {
    pub fn new(command: impl Into<PathBuf>, oauth_token: Option<String>) -> Self {
        Self {
            command: command.into(),
            oauth_token,
        }
    }

    fn redact(&self, text: &str) -> String {
        let cleaned = match &self.oauth_token {
            Some(token) if !token.is_empty() => text.replace(token.as_str(), "***"),
            _ => text.to_owned(),
        };
        cleaned.chars().take(MAX_ERROR_CHARS).collect()
    }

    fn command(&self, request: &AgentRequest, sandbox: &std::path::Path) -> Command {
        let mut command = Command::new(&self.command);
        command
            .args(["-p", &request.prompt])
            .args(["--output-format", "json"])
            .arg("--no-session-persistence")
            .args(["--permission-mode", "dontAsk"])
            .args(["--tools", ""]);
        if let Some(model) = &request.model {
            command.args(["--model", model]);
        }
        command
            .current_dir(sandbox)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        match &self.oauth_token {
            Some(token) => {
                command
                    .env("CLAUDE_CODE_OAUTH_TOKEN", token)
                    .env("HOME", sandbox);
            }
            None => {
                if let Some(home) = std::env::var_os("HOME") {
                    command.env("HOME", home);
                }
            }
        }
        command
    }

    async fn execute(
        &self,
        request: &AgentRequest,
        sandbox: &std::path::Path,
    ) -> Result<AgentOutput, AgentError> {
        let mut child = self
            .command(request, sandbox)
            .spawn()
            .map_err(|err| AgentError::Unavailable(err.to_string()))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(AgentError::Failed("flux de sortie indisponibles".into()));
        };
        let drain = |stream: Box<dyn tokio::io::AsyncRead + Send + Unpin>, limit: u64| {
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                stream
                    .take(limit)
                    .read_to_end(&mut buffer)
                    .await
                    .map(|_| buffer)
            })
        };
        let out_task = drain(Box::new(stdout), MAX_OUTPUT_BYTES + 1);
        let err_task = drain(Box::new(stderr), MAX_OUTPUT_BYTES);
        let read = async {
            let failed = |e: &dyn std::fmt::Display| AgentError::Failed(e.to_string());
            let out = out_task
                .await
                .map_err(|e| failed(&e))?
                .map_err(|e| failed(&e))?;
            if out.len() as u64 > MAX_OUTPUT_BYTES {
                let _ = child.kill().await;
                return Err(AgentError::Failed(
                    "sortie de l'agent trop volumineuse".into(),
                ));
            }
            let err = err_task
                .await
                .map_err(|e| failed(&e))?
                .map_err(|e| failed(&e))?;
            let status = child.wait().await.map_err(|e| failed(&e))?;
            Ok::<_, AgentError>((status, out, err))
        };
        let (status, out, err) = tokio::time::timeout(request.timeout, read)
            .await
            .map_err(|_| AgentError::Timeout)??;
        match serde_json::from_slice::<Value>(&out) {
            Ok(json) => self.interpret(&json),
            Err(_) if !status.success() => Err(AgentError::Failed(
                self.redact(&String::from_utf8_lossy(&err)),
            )),
            Err(_) => Err(AgentError::Failed("sortie de l'agent illisible".into())),
        }
    }

    fn interpret(&self, json: &Value) -> Result<AgentOutput, AgentError> {
        let text = json
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if json
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(AgentError::Failed(self.redact(text)));
        }
        Ok(AgentOutput {
            result: text.to_owned(),
            cost_usd: json
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            turns: json.get("num_turns").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

impl AgentRunner for ClaudeCli {
    fn run<'a>(&'a self, request: &'a AgentRequest) -> AgentFuture<'a> {
        Box::pin(async move {
            let sandbox = std::env::temp_dir().join(format!("sw-agent-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&sandbox).map_err(|e| AgentError::Unavailable(e.to_string()))?;
            let outcome = self.execute(request, &sandbox).await;
            let _ = std::fs::remove_dir_all(&sandbox);
            outcome
        })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const TOKEN: &str = "sk-ant-oat01-SECRETTOKEN";

    // Faux binaire : consigne ses arguments, son environnement et son dossier courant.
    fn fake(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("claude");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{d}/args\"\nenv > \"{d}/env\"\npwd > \"{d}/cwd\"\n{body}\n",
            d = dir.display()
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn request(timeout_ms: u64) -> AgentRequest {
        AgentRequest {
            prompt: "Résume cette alerte".into(),
            model: None,
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    const OK: &str =
        r#"echo '{"result":"pong","is_error":false,"total_cost_usd":0.01,"num_turns":1}'"#;

    #[tokio::test]
    async fn a_successful_run_returns_text_cost_and_turns() {
        let dir = tempfile::tempdir().unwrap();
        let runner = ClaudeCli::new(fake(dir.path(), OK), Some(TOKEN.into()));
        let output = runner.run(&request(5000)).await.unwrap();
        assert_eq!(
            output,
            AgentOutput {
                result: "pong".into(),
                cost_usd: 0.01,
                turns: 1
            }
        );
    }

    #[tokio::test]
    async fn the_cli_gets_the_prompt_and_no_tools() {
        let dir = tempfile::tempdir().unwrap();
        let runner = ClaudeCli::new(fake(dir.path(), OK), None);
        let mut req = request(5000);
        req.model = Some("sonnet".into());
        runner.run(&req).await.unwrap();
        let args: Vec<String> = std::fs::read_to_string(dir.path().join("args"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            args,
            [
                "-p",
                "Résume cette alerte",
                "--output-format",
                "json",
                "--no-session-persistence",
                "--permission-mode",
                "dontAsk",
                "--tools",
                "",
                "--model",
                "sonnet"
            ]
        );
        assert!(!args.iter().any(|a| a.contains("dangerously")));
    }

    #[tokio::test]
    async fn the_environment_is_cleared_and_the_home_is_a_sandbox_when_a_token_is_used() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY-free : la variable est posée uniquement pour vérifier qu'elle ne fuit pas.
        let runner = ClaudeCli::new(fake(dir.path(), OK), Some(TOKEN.into()));
        runner.run(&request(5000)).await.unwrap();
        let env = std::fs::read_to_string(dir.path().join("env")).unwrap();
        assert!(env.contains(&format!("CLAUDE_CODE_OAUTH_TOKEN={TOKEN}")));
        assert!(!env.contains("SW_JWT_SECRET") && !env.contains("ANTHROPIC_API_KEY"));
        let cwd = std::fs::read_to_string(dir.path().join("cwd")).unwrap();
        let home = env.lines().find_map(|l| l.strip_prefix("HOME=")).unwrap();
        assert_eq!(
            home.trim_start_matches("/private"),
            cwd.trim().trim_start_matches("/private")
        );
        assert!(cwd.contains("sw-agent-"));
    }

    #[tokio::test]
    async fn the_sandbox_is_removed_after_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let runner = ClaudeCli::new(fake(dir.path(), OK), None);
        runner.run(&request(5000)).await.unwrap();
        let cwd = std::fs::read_to_string(dir.path().join("cwd")).unwrap();
        assert!(!std::path::Path::new(cwd.trim()).exists());
    }

    #[tokio::test]
    async fn an_error_result_is_a_failure_with_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"echo '{"result":"Not logged in","is_error":true}'"#;
        let runner = ClaudeCli::new(fake(dir.path(), body), None);
        assert_eq!(
            runner.run(&request(5000)).await,
            Err(AgentError::Failed("Not logged in".into()))
        );
    }

    #[tokio::test]
    async fn a_slow_agent_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let runner = ClaudeCli::new(fake(dir.path(), "sleep 5"), None);
        let started = std::time::Instant::now();
        assert_eq!(runner.run(&request(300)).await, Err(AgentError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn oversized_output_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let body = "head -c 400000 /dev/zero | tr '\\0' 'a'";
        let runner = ClaudeCli::new(fake(dir.path(), body), None);
        let error = runner.run(&request(5000)).await.unwrap_err();
        assert!(matches!(error, AgentError::Failed(m) if m.contains("volumineuse")));
    }

    #[tokio::test]
    async fn unreadable_output_and_stderr_failures_never_leak_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!("echo 'boom {TOKEN} fin' >&2\nexit 1");
        let runner = ClaudeCli::new(fake(dir.path(), &body), Some(TOKEN.into()));
        let AgentError::Failed(message) = runner.run(&request(5000)).await.unwrap_err() else {
            panic!("échec attendu");
        };
        assert!(
            message.contains("boom") && !message.contains("SECRETTOKEN"),
            "{message}"
        );
        let dir = tempfile::tempdir().unwrap();
        let runner = ClaudeCli::new(fake(dir.path(), "echo pas-du-json"), None);
        assert_eq!(
            runner.run(&request(5000)).await,
            Err(AgentError::Failed("sortie de l'agent illisible".into()))
        );
    }

    #[tokio::test]
    async fn a_missing_binary_is_unavailable() {
        let runner = ClaudeCli::new("/nonexistent/claude", None);
        assert!(matches!(
            runner.run(&request(1000)).await,
            Err(AgentError::Unavailable(_))
        ));
    }

    #[test]
    fn debug_hides_the_token() {
        let text = format!("{:?}", ClaudeCli::new("claude", Some(TOKEN.into())));
        assert!(!text.contains("SECRETTOKEN") && text.contains("***"));
    }
}
