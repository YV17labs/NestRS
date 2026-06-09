use std::fs;
use std::path::Path;

use crate::error::{CliError, CliResult};
use crate::naming::Names;

pub fn write_file(path: &Path, contents: &str) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(CliError::Io)?;
    }
    if path.exists() {
        return Err(CliError::AlreadyExists(path.to_path_buf()));
    }
    fs::write(path, contents).map_err(CliError::Io)
}

pub fn render(template: &str, names: &Names) -> String {
    render_with_extra(template, names, &[])
}

pub fn render_with_extra(template: &str, names: &Names, extra: &[(&str, &str)]) -> String {
    let module = names.module();
    let service = names.service();
    let controller = names.controller();
    let http_module = names.http_module();

    let mut out = template
        .replace("{{kebab}}", &names.kebab)
        .replace("{{snake}}", &names.snake)
        .replace("{{pascal}}", &names.pascal)
        .replace("{{module}}", &module)
        .replace("{{service}}", &service)
        .replace("{{controller}}", &controller)
        .replace("{{http_module}}", &http_module);

    for (key, value) in extra {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

pub fn append_feature_mod(lib_rs: &Path, snake: &str) -> CliResult<()> {
    let source = fs::read_to_string(lib_rs).map_err(CliError::Io)?;
    let needle = format!("pub mod {snake};");
    if source.contains(&needle) {
        return Ok(());
    }

    let mut lines: Vec<&str> = source.lines().collect();
    let insert_at = lines
        .iter()
        .position(|line| line.starts_with("pub use "))
        .unwrap_or(lines.len());

    lines.insert(insert_at, &needle);
    let updated = format!("{}\n", lines.join("\n"));
    fs::write(lib_rs, updated).map_err(CliError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_placeholders() {
        let names = Names::parse("posts");
        let rendered = render("struct {{module}};", &names);
        assert_eq!(rendered, "struct PostsModule;");
    }
}
