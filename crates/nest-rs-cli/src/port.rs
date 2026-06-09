use std::fs;
use std::path::Path;

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};

const DEFAULT_HTTP_PORT: u16 = 3000;

/// Next HTTP listen port for a workspace app — scans `apps/*/src/module.rs`
/// for explicit `HttpConfig { port: … }` (and treats `for_root(None)` as the
/// base). The base is the workspace's `[workspace.metadata.nestrs] port-base`
/// when set, else 3000.
pub fn next_http_port(ws: &NestrsWorkspace) -> CliResult<u16> {
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
        let module_rs = entry.map_err(CliError::Io)?.path().join("src/module.rs");
        if module_rs.is_file() {
            let content = fs::read_to_string(&module_rs).map_err(CliError::Io)?;
            ports.extend(parse_ports(&content));
        }
    }

    Ok(ports)
}

fn parse_ports(content: &str) -> Vec<u16> {
    let mut ports = Vec::new();

    if content.contains("for_root(None)") {
        ports.push(DEFAULT_HTTP_PORT);
    }

    for line in content.lines() {
        let Some(rest) = line.split("port:").nth(1) else {
            continue;
        };
        let digits: String = rest
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(port) = digits.parse::<u16>() {
            ports.push(port);
        }
    }

    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_and_default_ports() {
        let content = r#"
HttpModule::for_root(None)
HttpModule::for_root(HttpConfig { port: 3002, ..Default::default() })
"#;
        let mut ports = parse_ports(content);
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports, vec![3000, 3002]);
    }

    #[test]
    fn next_port_after_existing_apps() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("apps");
        fs::create_dir_all(apps.join("auth/src")).unwrap();
        fs::create_dir_all(apps.join("api/src")).unwrap();
        fs::write(
            apps.join("auth/src/module.rs"),
            "HttpModule::for_root(HttpConfig { port: 3001, ..Default::default() })",
        )
        .unwrap();
        fs::write(
            apps.join("api/src/module.rs"),
            "HttpModule::for_root(HttpConfig { port: 3002, ..Default::default() })",
        )
        .unwrap();

        let used = collect_used_ports(&apps).unwrap();
        assert_eq!(used, vec![3001, 3002]);
        assert_eq!(used.into_iter().max().unwrap() + 1, 3003);
    }
}
