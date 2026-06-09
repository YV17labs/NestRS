//! Name derivation for the scaffolder.
//!
//! One input name (any case) → every identifier a generator needs: the
//! kebab/snake/pascal forms, the singular entity name (`users` → `User`),
//! the DTO names, and the per-transport module names.

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

/// Reject path segments that would escape the features workspace.
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
        format!("{}Jobs", self.pascal)
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

    pub fn create_input(&self) -> String {
        format!("Create{}Input", self.singular)
    }

    pub fn update_input(&self) -> String {
        format!("Update{}Input", self.singular)
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
        assert_eq!(names.create_input(), "CreatePostInput");
        assert_eq!(names.update_input(), "UpdatePostInput");
        assert_eq!(names.module_for(Transport::Http), "PostsHttpModule");
        assert_eq!(names.module_for(Transport::Graphql), "PostsGraphqlModule");
        assert_eq!(names.handler_for(Transport::Ws), "PostsGateway");
        assert_eq!(names.http_module(), "PostsHttpModule");
    }
}
