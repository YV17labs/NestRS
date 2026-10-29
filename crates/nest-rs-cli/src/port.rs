use std::fs;
use std::path::Path;

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};

const DEFAULT_HTTP_PORT: u16 = 3000;

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
/// the framework's default when that module mounts `HttpModule` at all.
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
        pinned_port(&module).or_else(|| module.contains("HttpModule").then_some(DEFAULT_HTTP_PORT))
    }))
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
        for app in ["auth", "api", "hello", "legacy", "cli"] {
            fs::create_dir_all(apps.join(app).join("src")).unwrap();
        }
        fs::write(apps.join("auth/.env"), "# its own\nACME_HTTP__PORT=3001\n").unwrap();
        fs::write(
            apps.join("auth/src/module.rs"),
            "HttpModule::for_root(None)",
        )
        .unwrap();
        fs::write(apps.join("api/.env"), "export NESTRS_HTTP__PORT=\"3004\"\n").unwrap();
        fs::write(
            apps.join("hello/src/module.rs"),
            "HttpModule::for_root(None)",
        )
        .unwrap();
        fs::write(
            apps.join("legacy/src/module.rs"),
            "HttpModule::for_root(HttpConfig { port: 3002, ..Default::default() })",
        )
        .unwrap();
        fs::write(apps.join("cli/src/module.rs"), "ConfigModule::for_root()").unwrap();

        let mut used = collect_used_ports(&apps).unwrap();
        // `read_dir` order is not guaranteed.
        used.sort_unstable();
        assert_eq!(used, vec![3000, 3001, 3002, 3004]);
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
