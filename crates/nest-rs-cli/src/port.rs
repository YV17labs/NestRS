use std::fs;
use std::path::Path;

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};
use crate::naming::Transport;

const DEFAULT_HTTP_PORT: u16 = 3000;

/// The framework modules that listen on HTTP, each importing `HttpModule`.
const MOUNTS_ON_HTTP: [&str; 7] = [
    "GraphqlModule",
    "HealthModule",
    "HttpModule",
    "McpModule",
    "OAuthResourceModule",
    "OpenApiModule",
    "StaticFilesModule",
];

/// Next HTTP listen port for a workspace app — one past the highest any app
/// under `apps/` takes. The base is the workspace's
/// `[workspace.metadata.nestrs] port-base` when set, else 3000.
pub(crate) fn next_http_port(ws: &NestrsWorkspace) -> CliResult<u16> {
    let base = ws.metadata.port_base;
    let used = collect_used_ports(&ws.apps_root())?;
    if used.is_empty() {
        return Ok(base);
    }
    Ok(used.into_iter().max().unwrap_or(base) + 1)
}

fn collect_used_ports(apps_root: &Path) -> CliResult<Vec<u16>> {
    let mut ports = Vec::new();
    if !apps_root.is_dir() {
        return Ok(ports);
    }

    for entry in fs::read_dir(apps_root).map_err(CliError::Io)? {
        ports.extend(app_port(&entry.map_err(CliError::Io)?.path())?);
    }

    Ok(ports)
}

/// The port the app in `app` listens on: its own `.env`'s
/// `<PREFIX>_HTTP__PORT`, else a port its `src/module.rs` pins in code, else
/// the framework's default when that module imports anything listening on
/// HTTP.
fn app_port(app: &Path) -> CliResult<Option<u16>> {
    let read = |path: &Path| -> CliResult<Option<String>> {
        if !path.is_file() {
            return Ok(None);
        }
        fs::read_to_string(path).map(Some).map_err(CliError::Io)
    };
    if let Some(port) = read(&app.join(".env"))?.as_deref().and_then(env_port) {
        return Ok(Some(port));
    }
    Ok(read(&app.join("src/module.rs"))?.and_then(|module| {
        pinned_port(&module).or_else(|| listens_on_http(&module).then_some(DEFAULT_HTTP_PORT))
    }))
}

/// Whether an app's root `module` imports anything listening on HTTP: a
/// framework module mounting there, or a feature's adapter of an edge served
/// over it (`BlogHttpModule`, `ChatWsModule`), which imports the transport.
fn listens_on_http(module: &str) -> bool {
    module
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .flat_map(|code| code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')))
        .any(|word| {
            MOUNTS_ON_HTTP.contains(&word)
                || word.strip_suffix("Module").is_some_and(|stem| {
                    Transport::ALL
                        .into_iter()
                        .filter(|transport| transport.serves_on_http())
                        .filter_map(|transport| stem.strip_suffix(transport.module_infix()))
                        .any(|feature| feature.starts_with(|c: char| c.is_ascii_uppercase()))
                })
        })
}

/// `<PREFIX>_HTTP__PORT` as a `.env` file assigns it, whatever the prefix.
fn env_port(contents: &str) -> Option<u16> {
    crate::dotenv::assignments(contents)
        .filter(|(key, _)| key.ends_with("_HTTP__PORT"))
        .find_map(|(_, value)| value.trim_matches(['"', '\'']).parse().ok())
}

/// A port pinned in code: `HttpConfig { port: … }`.
fn pinned_port(content: &str) -> Option<u16> {
    content.lines().find_map(|line| {
        let digits: String = line
            .split("port:")
            .nth(1)?
            .trim()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_s_port_is_its_own_env_s_then_its_pin_then_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("apps");
        let roots = [
            ("auth", "HttpModule::for_root(None)"),
            ("api", "BlogHttpModule"),
            (
                "legacy",
                "HttpModule::for_root(HttpConfig { port: 3002, ..Default::default() })",
            ),
            ("hello", "HttpModule::for_root(None)"),
            ("blog", "[BlogHttpModule]"),
            ("live", "[ChatWsModule, NotificationsWsModule]"),
            ("assistant", "[McpModule::for_root(opts), UsersMcpModule]"),
            ("graph", "[UsersGraphqlModule]"),
            (
                "probes",
                "[HealthModule, AudioQueueModule, RedisQueueModule]",
            ),
            (
                "jobs",
                "[AudioQueueModule, RedisQueueModule, AudioScheduleModule]",
            ),
            ("pushes", "// HttpModule\n[ChatModule, WsModule]"),
            ("cli", "ConfigModule::for_root()"),
        ];
        for (app, module) in roots {
            fs::create_dir_all(apps.join(app).join("src")).unwrap();
            fs::write(apps.join(app).join("src/module.rs"), module).unwrap();
        }
        fs::write(apps.join("auth/.env"), "# its own\nACME_HTTP__PORT=3001\n").unwrap();
        fs::write(apps.join("api/.env"), "export NESTRS_HTTP__PORT=\"3004\"\n").unwrap();

        let mut used = collect_used_ports(&apps).unwrap();
        // `read_dir` order is not guaranteed.
        used.sort_unstable();
        assert_eq!(
            used,
            vec![3000, 3000, 3000, 3000, 3000, 3000, 3001, 3002, 3004]
        );
    }

    #[test]
    fn a_commented_or_foreign_line_names_no_port() {
        assert_eq!(
            env_port("# NESTRS_HTTP__PORT=3009\nNESTRS_HTTP__PORTS=1\n"),
            None
        );
        assert_eq!(env_port("NESTRS_HTTP__PORT = 3009\n"), Some(3009));
    }
}
