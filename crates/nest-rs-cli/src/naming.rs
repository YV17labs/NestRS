//! Name derivation for the scaffolder.
//!
//! One input name (any case) → every identifier a generator needs: the
//! kebab/snake/pascal forms, the singular entity name (`users` → `User`),
//! the CRUD form names, and the per-transport module names.

/// The transports a feature can expose. Drives adapter folder names,
/// module struct names, and the access-graph imports a generator wires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Http,
    Graphql,
    Ws,
    Queue,
    Schedule,
    Mcp,
}

impl Transport {
    /// Adapter sub-folder under the feature root (`users/http/`).
    pub fn folder(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Graphql => "graphql",
            Self::Ws => "ws",
            Self::Queue => "queue",
            Self::Schedule => "schedule",
            Self::Mcp => "mcp",
        }
    }

    /// PascalCase infix used in the module name (`Users<Http>Module`).
    fn module_infix(self) -> &'static str {
        match self {
            Self::Http => "Http",
            Self::Graphql => "Graphql",
            Self::Ws => "Ws",
            Self::Queue => "Queue",
            Self::Schedule => "Schedule",
            Self::Mcp => "Mcp",
        }
    }

    /// File holding the handler for this transport (`controller.rs`, …).
    pub fn handler_file(self) -> &'static str {
        match self {
            Self::Http => "controller.rs",
            Self::Graphql => "resolver.rs",
            Self::Ws => "gateway.rs",
            Self::Queue => "processor.rs",
            Self::Schedule => "tasks.rs",
            Self::Mcp => "tool.rs",
        }
    }

    /// Module name of the handler file (`controller`, `resolver`, …).
    pub fn handler_mod(self) -> &'static str {
        self.handler_file().trim_end_matches(".rs")
    }
}

#[derive(Debug, Clone)]
pub struct Names {
    /// `blog-posts`
    pub kebab: String,
    /// `blog_posts`
    pub snake: String,
    /// `BlogPosts`
    pub pascal: String,
    /// `BlogPost` — naive singular of `pascal`, used for entity/DTO names.
    pub singular: String,
}

/// Reject path segments that would escape the features workspace, and names
/// whose derived kebab form would not be a valid crate/package identifier.
pub fn validate_feature_name(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("feature name must not be empty".into());
    }
    if trimmed.contains("..") || trimmed.contains('/') || trimmed.contains('\\') {
        return Err("feature name must not contain path separators".into());
    }
    if trimmed.starts_with('.') {
        return Err("feature name must not start with '.'".into());
    }
    // The derived kebab is the crate/package/module name, so it must be a valid
    // identifier — otherwise the scaffold fails the next `cargo check` (CLI-I6).
    validate_derived_kebab(&to_kebab(trimmed))?;
    Ok(())
}

/// A derived kebab name must be a valid crate/package name: start with a
/// lowercase ASCII letter, then only lowercase letters, digits, or hyphens.
/// Catches `nestrs new "Bad Name!"` (→ `bad-name!`) or a digit-led name before
/// it scaffolds a project that fails to compile (CLI-I6).
pub fn validate_derived_kebab(kebab: &str) -> Result<(), String> {
    if kebab.is_empty() {
        return Err("the name has no letters or digits to form a package name".into());
    }
    let starts_with_letter = kebab.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_valid = kebab
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !starts_with_letter || !rest_valid {
        return Err(format!(
            "`{kebab}` is not a valid package name — the name must reduce to one that starts \
             with a letter and uses only lowercase letters, digits, and hyphens"
        ));
    }
    Ok(())
}

impl Names {
    pub fn parse(raw: &str) -> Self {
        let kebab = to_kebab(raw);
        let snake = kebab.replace('-', "_");
        let pascal = to_pascal(&kebab);
        let singular = singularize(&pascal);
        Self {
            kebab,
            snake,
            pascal,
            singular,
        }
    }

    pub fn module(&self) -> String {
        format!("{}Module", self.pascal)
    }

    pub fn service(&self) -> String {
        format!("{}Service", self.pascal)
    }

    pub fn controller(&self) -> String {
        format!("{}Controller", self.pascal)
    }

    pub fn resolver(&self) -> String {
        format!("{}Resolver", self.pascal)
    }

    pub fn gateway(&self) -> String {
        format!("{}Gateway", self.pascal)
    }

    pub fn processor(&self) -> String {
        format!("{}Processor", self.pascal)
    }

    pub fn tasks(&self) -> String {
        format!("{}Tasks", self.pascal)
    }

    pub fn tool(&self) -> String {
        format!("{}Tool", self.singular)
    }

    /// Entity/wire-model name — singular Pascal (`users` → `User`).
    pub fn entity(&self) -> String {
        self.singular.clone()
    }

    /// SQL table name — singular snake (`users` → `user`, `blog_posts` → `blog_post`).
    pub fn table(&self) -> String {
        to_kebab(&self.singular).replace('-', "_")
    }

    /// Create form derived from the entity (`CreatePost`). No transfer suffix:
    /// a CRUD shape derived from the entity has no single boundary — it is the
    /// service's `Create` type, the GraphQL `input`, and the REST body at once —
    /// so it joins the entity exception and stays bare. Hand-written transfer
    /// objects keep their boundary suffix (`…Dto`/`…Input`/`…Command`).
    pub fn create_op(&self) -> String {
        format!("Create{}", self.singular)
    }

    /// Update form derived from the entity (`UpdatePost`). Bare, same rationale
    /// as [`create_op`](Self::create_op).
    pub fn update_op(&self) -> String {
        format!("Update{}", self.singular)
    }

    /// Default queue payload a `g queue` scaffold emits — an imperative
    /// **`Command`** ("do this work" → one handler), the common case. Verb-led
    /// per the convention; the developer renames it to the real action
    /// (`GenerateMediaVariantCommand`), or switches to an `…Event` (past tense)
    /// when publishing a fact to several consumers.
    pub fn command(&self) -> String {
        format!("Process{}Command", self.singular)
    }

    /// `Users<Transport>Module`, e.g. `UsersHttpModule`.
    pub fn module_for(&self, transport: Transport) -> String {
        format!("{}{}Module", self.pascal, transport.module_infix())
    }

    /// The handler struct name a given transport adapter declares.
    pub fn handler_for(&self, transport: Transport) -> String {
        match transport {
            Transport::Http => self.controller(),
            Transport::Graphql => self.resolver(),
            Transport::Ws => self.gateway(),
            Transport::Queue => self.processor(),
            Transport::Schedule => self.tasks(),
            Transport::Mcp => self.tool(),
        }
    }

    /// Shorthand for the HTTP adapter module name.
    pub fn http_module(&self) -> String {
        self.module_for(Transport::Http)
    }
}

/// Placement for a boundary object that lives at the feature **port**, mirroring
/// the entity rule: a lone instance lives in `<role>.rs`; two or more split into
/// a pluralized `<role>s/` directory with one `<stem>_<role>.rs` per type,
/// re-exported flat by `<role>s/mod.rs`. `stem` is the snake_case type name
/// *without* the role suffix (`LoginDto` → `login`, `GenerateMediaVariantCommand`
/// → `generate_media_variant`). The boundary picks the role word — REST body
/// `dto`, imperative queue payload `command`, published-fact queue payload
/// `event` (see [`command_file`]).
fn port_role_file(role: &str, stem: &str, total: usize) -> String {
    if total <= 1 {
        format!("{role}.rs")
    } else {
        format!("{role}s/{stem}_{role}.rs")
    }
}

/// File holding an **imperative queue payload** (`Command` — "do this work",
/// one handler): one → `command.rs`, 2+ → `commands/<stem>_command.rs`. The
/// payload is a producer↔worker contract, so it lives at the port; the
/// `queue/` adapter's `processor.rs` imports it. The single-`command.rs` form
/// is what `g queue` emits today (via [`generate::adapter`](crate::commands));
/// the `commands/` directory form is the placement authority for the
/// multi-payload case.
pub fn command_file(stem: &str, total: usize) -> String {
    port_role_file("command", stem, total)
}

fn to_kebab(raw: &str) -> String {
    let mut out = String::new();
    for (i, ch) in raw.chars().enumerate() {
        if ch.is_whitespace() || ch == '_' {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
            continue;
        }
        if ch.is_uppercase() {
            if i > 0 && !out.ends_with('-') {
                out.push('-');
            }
            out.extend(ch.to_lowercase());
        } else if ch == '-' {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(ch);
        }
    }
    out.trim_matches('-').to_string()
}

fn to_pascal(kebab: &str) -> String {
    kebab
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut head = first.to_uppercase().to_string();
                    head.push_str(chars.as_str());
                    head
                }
            }
        })
        .collect()
}

/// Naive English singularization over the last word of a PascalCase name.
/// Good enough for identifiers: `Users`→`User`, `Categories`→`Category`,
/// `Statuses`→`Status`. Already-singular words pass through unchanged.
fn singularize(pascal: &str) -> String {
    if pascal.is_empty() {
        return pascal.to_string();
    }

    let lower = pascal.to_lowercase();
    if lower.ends_with("ies") {
        // `Categories` → `Category` (keep original casing of the stem).
        return format!("{}y", &pascal[..pascal.len() - 3]);
    }
    for suffix in ["ses", "xes", "zes", "ches", "shes"] {
        if lower.ends_with(suffix) {
            // `statuses` → `status`, `boxes` → `box`
            let keep = pascal.len() - 2;
            return pascal[..keep].to_string();
        }
    }
    if lower.ends_with("ss") {
        // `address` is singular already.
        return pascal.to_string();
    }
    if let Some(stripped) = pascal.strip_suffix('s') {
        return stripped.to_string();
    }
    pascal.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_feature_names() {
        assert!(validate_feature_name("/tmp/pwn").is_err());
        assert!(validate_feature_name("../escape").is_err());
        assert!(validate_feature_name("valid_name").is_ok());
    }

    #[test]
    fn rejects_names_that_derive_an_invalid_package_name() {
        // `!` survives kebab derivation → `bad-name!`, which won't compile as a
        // crate name — the scaffold must reject it up front (CLI-I6).
        assert!(validate_feature_name("Bad Name!").is_err());
        assert!(validate_feature_name("has space ok").is_ok()); // → has-space-ok
        assert!(validate_derived_kebab("bad-name!").is_err());
        assert!(
            validate_derived_kebab("123-start").is_err(),
            "must start with a letter"
        );
        assert!(validate_derived_kebab("").is_err());
        assert!(validate_derived_kebab("good-name").is_ok());
        assert!(validate_derived_kebab("blog-posts2").is_ok());
    }

    #[test]
    fn parses_kebab_names() {
        let names = Names::parse("my-api");
        assert_eq!(names.kebab, "my-api");
        assert_eq!(names.snake, "my_api");
        assert_eq!(names.pascal, "MyApi");
        assert_eq!(names.module(), "MyApiModule");
    }

    #[test]
    fn parses_snake_names() {
        let names = Names::parse("blog_posts");
        assert_eq!(names.kebab, "blog-posts");
        assert_eq!(names.snake, "blog_posts");
        assert_eq!(names.pascal, "BlogPosts");
        assert_eq!(names.singular, "BlogPost");
    }

    #[test]
    fn singularizes_entity_names() {
        assert_eq!(Names::parse("users").entity(), "User");
        assert_eq!(Names::parse("categories").entity(), "Category");
        assert_eq!(Names::parse("statuses").entity(), "Status");
        assert_eq!(Names::parse("post").entity(), "Post");
        assert_eq!(Names::parse("address").entity(), "Address");
    }

    #[test]
    fn dto_and_transport_module_names() {
        let names = Names::parse("posts");
        // CRUD forms derived from the entity carry no transfer suffix.
        assert_eq!(names.create_op(), "CreatePost");
        assert_eq!(names.update_op(), "UpdatePost");
        // A scaffolded queue payload defaults to an imperative, verb-led Command.
        assert_eq!(names.command(), "ProcessPostCommand");
        assert_eq!(names.processor(), "PostsProcessor");
        assert_eq!(names.module_for(Transport::Http), "PostsHttpModule");
        assert_eq!(names.module_for(Transport::Graphql), "PostsGraphqlModule");
        assert_eq!(names.handler_for(Transport::Ws), "PostsGateway");
        assert_eq!(names.http_module(), "PostsHttpModule");
    }

    #[test]
    fn command_file_layout_mirrors_the_dto_rule() {
        // A lone imperative payload lives directly in `command.rs`.
        assert_eq!(command_file("transcode", 1), "command.rs");
        // Two or more split into a pluralized `commands/` directory, one
        // `<stem>_command.rs` per type — simple and multi-word stems.
        assert_eq!(
            command_file("transcode", 2),
            "commands/transcode_command.rs"
        );
        assert_eq!(
            command_file("generate_media_variant", 2),
            "commands/generate_media_variant_command.rs"
        );
    }
}
