#[derive(Debug, Clone)]
pub struct Names {
    pub kebab: String,
    pub snake: String,
    pub pascal: String,
}

impl Names {
    pub fn parse(raw: &str) -> Self {
        let kebab = to_kebab(raw);
        let snake = kebab.replace('-', "_");
        let pascal = to_pascal(&kebab);
        Self {
            kebab,
            snake,
            pascal,
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

    pub fn http_module(&self) -> String {
        format!("{}HttpModule", self.pascal)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
