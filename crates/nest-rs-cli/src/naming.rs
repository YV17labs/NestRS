//! Name derivation for the scaffolder.
//!
//! One input name (any case) → every identifier a generator needs: the
//! kebab/snake/pascal forms, the singular entity name (`users` → `User`),
//! the CRUD form names, and the per-transport module names.

use std::collections::BTreeMap;
use std::sync::LazyLock;

/// The transports a feature can expose. Drives adapter folder names,
/// module struct names, and the access-graph imports a generator wires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    Http,
    Graphql,
    Ws,
    Queue,
    Schedule,
    Mcp,
    Events,
}

impl Transport {
    /// Every transport, for a check over the whole adapter surface. Test-only.
    #[cfg(test)]
    pub(crate) const ALL: [Transport; 7] = [
        Self::Http,
        Self::Graphql,
        Self::Ws,
        Self::Queue,
        Self::Schedule,
        Self::Mcp,
        Self::Events,
    ];

    /// Adapter sub-folder under the feature root (`users/http/`).
    pub(crate) fn folder(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Graphql => "graphql",
            Self::Ws => "ws",
            Self::Queue => "queue",
            Self::Schedule => "schedule",
            Self::Mcp => "mcp",
            Self::Events => "events",
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
            Self::Events => "Events",
        }
    }

    /// File holding the handler for this transport (`controller.rs`, …).
    pub(crate) fn handler_file(self) -> &'static str {
        match self {
            Self::Http => "controller.rs",
            Self::Graphql => "resolver.rs",
            Self::Ws => "gateway.rs",
            Self::Queue => "processor.rs",
            Self::Schedule => "tasks.rs",
            Self::Mcp => "tool.rs",
            Self::Events => "listener.rs",
        }
    }

    /// Module name of the handler file (`controller`, `resolver`, …).
    pub(crate) fn handler_mod(self) -> &'static str {
        self.handler_file().trim_end_matches(".rs")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Names {
    /// `blog-posts`
    pub kebab: String,
    /// `blog_posts`
    pub snake: String,
    /// `BlogPosts`
    pub pascal: String,
    /// `BlogPost` — naive singular of `pascal`, used for entity/DTO names.
    pub singular: String,
}

/// The architecture rules this CLI ships — the same bytes `shared::AGENTS_BODY`
/// embeds.
static ARCHITECTURE_RULES: &str = include_str!("templates/architecture.md");

/// The structural vocabulary, word → the category that claims it, **derived**
/// from [`ARCHITECTURE_RULES`] rather than transcribed.
///
/// The rules file states it as one fenced block under *Reserved vocabulary*,
/// one row per category, continuation rows indented — so a row's first token is
/// its category when the line starts flush, and every other token is a word.
static RESERVED: LazyLock<BTreeMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut out = BTreeMap::new();
    let block = ARCHITECTURE_RULES
        .split_once("## Reserved vocabulary")
        .and_then(|(_, rest)| rest.split_once("```"))
        .and_then(|(_, rest)| rest.split_once("```"))
        .map(|(block, _)| block)
        .unwrap_or_default();

    let mut category = "";
    for line in block.lines().filter(|line| !line.trim().is_empty()) {
        let mut words = line.split_whitespace();
        if !line.starts_with(char::is_whitespace)
            && let Some(head) = words.next()
        {
            category = head;
        }
        for word in words {
            out.entry(word).or_insert(category);
        }
    }
    out
});

/// The category claiming `word`, or `None` when the layout has no meaning for
/// it.
pub(crate) fn reserved_category(word: &str) -> Option<&'static str> {
    RESERVED.get(word).copied()
}

/// How a category's words are already spent, in the sentence a refusal reads.
fn reserved_role(category: &str, word: &str) -> String {
    match category {
        "structure" => "a workspace directory".to_string(),
        "roles" => format!("the name of a role file (`{word}.rs`)"),
        "plurals" => format!("the name of a pluralized role folder (`{word}/`)"),
        "edges" => format!("the name of a transport adapter folder (`{word}/`)"),
        _ => "part of the structural vocabulary".to_string(),
    }
}

/// Reject path segments that would escape the features workspace, names whose
/// derived kebab form would not be a valid crate/package identifier, and names
/// the layout has already spent.
pub(crate) fn validate_feature_name(raw: &str) -> Result<(), String> {
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
    // identifier.
    let kebab = to_kebab(trimmed);
    validate_derived_kebab(&kebab)?;
    validate_not_reserved(&kebab)?;
    Ok(())
}

/// Refuse a name the structural vocabulary has already spent — `ModuleModule`,
/// or an `HttpModule` colliding with `nest_rs::http::HttpModule`.
fn validate_not_reserved(kebab: &str) -> Result<(), String> {
    let Some(category) = reserved_category(kebab) else {
        return Ok(());
    };
    Err(format!(
        "`{kebab}` is reserved — the layout already spends it as {}, so a module of that name \
         makes every path that mentions it ambiguous. Pick the domain word instead: a module \
         about desktop applications is `programs`, not `apps`.",
        reserved_role(category, kebab),
    ))
}

/// A derived kebab name must be a valid crate/package name: start with a
/// lowercase ASCII letter, then only lowercase letters, digits, or hyphens.
pub(crate) fn validate_derived_kebab(kebab: &str) -> Result<(), String> {
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
    pub(crate) fn parse(raw: &str) -> Self {
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

    pub(crate) fn module(&self) -> String {
        format!("{}Module", self.pascal)
    }

    pub(crate) fn service(&self) -> String {
        format!("{}Service", self.pascal)
    }

    pub(crate) fn controller(&self) -> String {
        format!("{}Controller", self.pascal)
    }

    pub(crate) fn resolver(&self) -> String {
        format!("{}Resolver", self.pascal)
    }

    pub(crate) fn gateway(&self) -> String {
        format!("{}Gateway", self.pascal)
    }

    pub(crate) fn processor(&self) -> String {
        format!("{}Processor", self.pascal)
    }

    /// The `#[queue]` marker both sides of the queue import — the wire name and
    /// the payload type in one artifact.
    pub(crate) fn queue(&self) -> String {
        format!("{}Queue", self.pascal)
    }

    pub(crate) fn tasks(&self) -> String {
        format!("{}Tasks", self.pascal)
    }

    pub(crate) fn tool(&self) -> String {
        format!("{}Tool", self.pascal)
    }

    pub(crate) fn listener(&self) -> String {
        format!("{}Listener", self.pascal)
    }

    /// The fact the `g events` skeleton listens for — past tense, at the port.
    pub(crate) fn event(&self) -> String {
        format!("{}ChangedEvent", self.singular)
    }

    /// Entity/wire-model name — singular Pascal (`users` → `User`).
    pub(crate) fn entity(&self) -> String {
        self.singular.clone()
    }

    /// SQL table name — singular snake (`users` → `user`, `blog_posts` → `blog_post`).
    pub(crate) fn table(&self) -> String {
        to_kebab(&self.singular).replace('-', "_")
    }

    /// Create form derived from the entity (`CreatePost`), bare: it is the
    /// service's `Create` type, the GraphQL input and the REST body at once.
    pub(crate) fn create_op(&self) -> String {
        format!("Create{}", self.singular)
    }

    /// Update form derived from the entity (`UpdatePost`). Bare, same rationale
    /// as [`create_op`](Self::create_op).
    pub(crate) fn update_op(&self) -> String {
        format!("Update{}", self.singular)
    }

    /// Default queue payload a `g queue` scaffold emits — an imperative, verb-led
    /// **`Command`** the developer renames to the real action.
    pub(crate) fn command(&self) -> String {
        format!("Process{}Command", self.singular)
    }

    /// `Users<Transport>Module`, e.g. `UsersHttpModule`.
    pub(crate) fn module_for(&self, transport: Transport) -> String {
        format!("{}{}Module", self.pascal, transport.module_infix())
    }

    /// The handler struct name a given transport adapter declares.
    pub(crate) fn handler_for(&self, transport: Transport) -> String {
        match transport {
            Transport::Http => self.controller(),
            Transport::Graphql => self.resolver(),
            Transport::Ws => self.gateway(),
            Transport::Queue => self.processor(),
            Transport::Schedule => self.tasks(),
            Transport::Mcp => self.tool(),
            Transport::Events => self.listener(),
        }
    }

    /// Shorthand for the HTTP adapter module name.
    pub(crate) fn http_module(&self) -> String {
        self.module_for(Transport::Http)
    }
}

/// Verbs a migration name leads with. Exactly one is stripped — a table
/// genuinely called `add_ons` survives `create_add_ons`.
const MIGRATION_VERBS: &[&str] = &[
    "create", "add", "alter", "change", "drop", "delete", "remove", "rename", "update", "modify",
    "init", "backfill", "make",
];

/// Words that introduce the table a migration acts *on* — everything after the
/// last one names it (`add_status_to_posts`, `create_index_on_users`).
const MIGRATION_TARGET_WORDS: &[&str] = &["to", "from", "on", "in", "into", "for"];

/// The table a migration name is about, as [`Names`]: `create_widgets` →
/// `widgets`, `add_status_to_posts` → `posts`, `drop_orgs_table` → `orgs`. The
/// identifier enum is the **table**, not the migration: `DeriveIden` snake-cases
/// it straight into the SQL. A name with nothing left to strip stands as its own
/// subject.
pub(crate) fn migration_subject(raw: &str) -> Names {
    let whole = Names::parse(raw);
    let all: Vec<&str> = whole.snake.split('_').filter(|t| !t.is_empty()).collect();
    let mut tokens: &[&str] = &all;

    if let Some(idx) = tokens
        .iter()
        .rposition(|t| MIGRATION_TARGET_WORDS.contains(t))
        && idx + 1 < tokens.len()
    {
        tokens = &tokens[idx + 1..];
    } else if let [verb, rest @ ..] = tokens
        && !rest.is_empty()
        && MIGRATION_VERBS.contains(verb)
    {
        tokens = rest;
    }
    if let [rest @ .., "table"] = tokens
        && !rest.is_empty()
    {
        tokens = rest;
    }

    if tokens.is_empty() {
        whole
    } else {
        Names::parse(&tokens.join("_"))
    }
}

/// Placement for a boundary object at the feature **port**: one lives in
/// `<role>.rs`; two or more in `<role>s/<stem>_<role>.rs`, re-exported flat by
/// `<role>s/mod.rs`. `stem` is the snake_case type name without the role suffix
/// (`LoginDto` → `login`).
fn port_role_file(role: &str, stem: &str, total: usize) -> String {
    if total <= 1 {
        format!("{role}.rs")
    } else {
        format!("{role}s/{stem}_{role}.rs")
    }
}

/// File holding an **imperative queue payload**, at the port: one →
/// `command.rs`, 2+ → `commands/<stem>_command.rs`.
pub(crate) fn command_file(stem: &str, total: usize) -> String {
    port_role_file("command", stem, total)
}

pub(crate) fn to_kebab(raw: &str) -> String {
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

    /// A failed scrape is silent — every reserved word accepted — so each
    /// category is asserted.
    #[test]
    fn derives_every_reserved_category_from_the_rules_file() {
        for (word, category) in [
            ("apps", "structure"),
            ("crates", "structure"),
            ("service", "roles"),
            ("module", "roles"),
            ("entity", "roles"),
            ("services", "plurals"),
            ("dtos", "plurals"),
            ("http", "edges"),
            ("graphql", "edges"),
        ] {
            assert_eq!(
                reserved_category(word),
                Some(category),
                "`{word}` must be scraped from architecture.md as {category}",
            );
        }
        // `events` is an edge and never a plural folder: one folder name, one meaning.
        assert_eq!(reserved_category("events"), Some("edges"));
        assert_eq!(reserved_category("programs"), None);
        assert!(validate_not_reserved("apps").is_err());
        assert!(validate_not_reserved("programs").is_ok());
    }

    #[test]
    fn rejects_path_traversal_feature_names() {
        assert!(validate_feature_name("/tmp/pwn").is_err());
        assert!(validate_feature_name("../escape").is_err());
        assert!(validate_feature_name("valid_name").is_ok());
    }

    #[test]
    fn rejects_names_that_derive_an_invalid_package_name() {
        // `!` survives kebab derivation → `bad-name!`.
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
        assert_eq!(names.create_op(), "CreatePost");
        assert_eq!(names.update_op(), "UpdatePost");
        assert_eq!(names.command(), "ProcessPostCommand");
        assert_eq!(names.processor(), "PostsProcessor");
        assert_eq!(names.module_for(Transport::Http), "PostsHttpModule");
        assert_eq!(names.module_for(Transport::Graphql), "PostsGraphqlModule");
        assert_eq!(names.handler_for(Transport::Ws), "PostsGateway");
        assert_eq!(names.http_module(), "PostsHttpModule");
    }

    #[test]
    fn migration_names_resolve_to_the_table_they_touch() {
        // `DeriveIden` would turn a `CreateWidgets` enum into a `create_widgets` table.
        let subject = migration_subject("create_widgets");
        assert_eq!(subject.singular, "Widget");
        assert_eq!(subject.table(), "widget");

        for (name, entity) in [
            ("create_org", "Org"),
            ("add_posts", "Post"),
            ("drop_widgets", "Widget"),
            ("alter_blog_posts", "BlogPost"),
            ("rename_categories", "Category"),
            ("create_users_table", "User"),
            ("backfill_statuses", "Status"),
        ] {
            assert_eq!(migration_subject(name).singular, entity, "{name}");
        }

        assert_eq!(migration_subject("add_status_to_posts").singular, "Post");
        assert_eq!(migration_subject("create_index_on_users").singular, "User");
        assert_eq!(
            migration_subject("drop_legacy_column_from_orgs").singular,
            "Org"
        );

        assert_eq!(migration_subject("create_add_ons").singular, "AddOn");
        assert_eq!(migration_subject("init").singular, "Init");
        assert_eq!(migration_subject("widgets").singular, "Widget");
    }

    #[test]
    fn command_file_layout_mirrors_the_dto_rule() {
        assert_eq!(command_file("transcode", 1), "command.rs");
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
