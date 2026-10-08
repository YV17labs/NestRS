//! [`RoutePath`] — a `#[routes]` path read with poem's own grammar, so the
//! address the macro checks is the address poem mounts.
//!
//! The grammar is poem 3.1's `Route::at` — `normalize_path` then
//! `parse_path_segments` — plus the one rule this framework adds before routing,
//! the edge's trailing-slash trim:
//!
//! - runs of `/` collapse, and a missing leading `/` is added (poem);
//! - a trailing `/` is not part of the address, except on the root (the edge);
//! - `:name` is a parameter, `:name<re>` and `<re>` a parameter constrained by a
//!   regular expression, `*` and `*name` the rest of the path (poem). Anything
//!   else is literal text — `{name}` included, which poem does not read as a
//!   parameter.
//!
//! The **identity** drops parameter names, which decide nothing about routing.
//! Pinned against poem in `nest-rs-http/tests/integration/controller.rs`.

/// One route path, as poem mounts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePath {
    identity: String,
    mount: String,
}

enum Segment<'a> {
    Static(&'a str),
    Param(&'a str),
    Regex(Option<&'a str>, &'a str),
    CatchAll(Option<&'a str>),
}

impl RoutePath {
    /// Read `written` with poem's grammar, or say why poem would refuse it at
    /// boot — the clause after a colon.
    pub fn parse(written: &str) -> Result<Self, String> {
        let normalized = normalize(written);
        let mut segments = segments(&normalized)?;
        // The edge trims a request's trailing slash before routing, so a route
        // declared with one names the address without it.
        if normalized != "/"
            && let Some(Segment::Static(text)) = segments.last_mut()
            && let Some(trimmed) = text.strip_suffix('/')
        {
            if trimmed.is_empty() {
                segments.pop();
            } else {
                *text = trimmed;
            }
        }
        let (mut identity, mut mount) = (String::new(), String::new());
        for segment in &segments {
            match segment {
                Segment::Static(text) => {
                    identity.push_str(text);
                    mount.push_str(text);
                }
                Segment::Param(name) => {
                    identity.push(':');
                    mount.push(':');
                    mount.push_str(name);
                }
                Segment::Regex(name, re) => {
                    identity.push('<');
                    identity.push_str(re);
                    identity.push('>');
                    if let Some(name) = name {
                        mount.push(':');
                        mount.push_str(name);
                    }
                    mount.push('<');
                    mount.push_str(re);
                    mount.push('>');
                }
                Segment::CatchAll(name) => {
                    identity.push('*');
                    mount.push('*');
                    mount.push_str(name.unwrap_or_default());
                }
            }
        }
        if identity.is_empty() {
            identity.push('/');
            mount.push('/');
        }
        Ok(Self { identity, mount })
    }

    /// What makes two routes one address: the path with every parameter's name
    /// dropped — `/users/:id/` and `users/:other` are both `/users/:`.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The path poem is handed: normalized, with its parameters' names — the
    /// address served, logged and documented.
    pub fn mount(&self) -> &str {
        &self.mount
    }
}

/// poem's `normalize_path`, which runs before anything is parsed.
fn normalize(written: &str) -> String {
    let mut out = String::with_capacity(written.len() + 1);
    if !written.starts_with('/') {
        out.push('/');
    }
    for c in written.chars() {
        if c == '/' && out.ends_with('/') {
            continue;
        }
        out.push(c);
    }
    out
}

/// poem's `parse_path_segments`, with its three refusals worded.
fn segments(path: &str) -> Result<Vec<Segment<'_>>, String> {
    let bytes = path.as_bytes();
    let mut segments = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b':' => {
                i += 1;
                let start = i;
                while i < bytes.len() && !matches!(bytes[i], b'/' | b'<' | b'*') {
                    i += 1;
                }
                let name = &path[start..i];
                if name.is_empty() {
                    return Err(
                        "a `:` begins a parameter and has to be followed by its name".into(),
                    );
                }
                if i < bytes.len() && bytes[i] == b'<' {
                    i += 1;
                    segments.push(Segment::Regex(Some(name), regex(path, &mut i)?));
                } else {
                    segments.push(Segment::Param(name));
                }
            }
            b'*' => {
                let name = &path[i + 1..];
                segments.push(Segment::CatchAll((!name.is_empty()).then_some(name)));
                break;
            }
            b'<' => {
                i += 1;
                segments.push(Segment::Regex(None, regex(path, &mut i)?));
            }
            _ => {
                let start = i;
                while i < bytes.len() && !matches!(bytes[i], b':' | b'*' | b'<') {
                    i += 1;
                }
                segments.push(Segment::Static(&path[start..i]));
            }
        }
    }
    Ok(segments)
}

fn regex<'a>(path: &'a str, i: &mut usize) -> Result<&'a str, String> {
    let start = *i;
    match path[start..].find('>') {
        Some(0) => Err("a `<…>` constraint has to hold a regular expression".into()),
        Some(len) => {
            *i = start + len + 1;
            Ok(&path[start..start + len])
        }
        None => Err("a `<` opens a regular expression that no `>` closes".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &str) -> (String, String) {
        let route = RoutePath::parse(path).unwrap_or_else(|why| panic!("{path}: {why}"));
        (route.identity().to_owned(), route.mount().to_owned())
    }

    #[test]
    fn spellings_of_one_address_share_an_identity() {
        for (a, b) in [
            ("/t", "t"),
            ("/u", "/u/"),
            ("/a//b", "/a/b"),
            ("/q/:id", "/q/:other"),
            ("/q/:id/", "q/:other"),
            ("/n/:id<\\d+>", "/n/<\\d+>"),
            ("/f/*", "/f/*rest"),
            ("/", ""),
            ("/", "//"),
        ] {
            assert_eq!(read(a).0, read(b).0, "{a} / {b}");
        }
    }

    #[test]
    fn addresses_poem_keeps_apart_do_not() {
        for (a, b) in [
            ("/Users", "/users"),
            ("/q/:id", "/q/:id/x"),
            ("/n/:id", "/n/:id<\\d+>"),
            ("/n/<\\d+>", "/n/<[a-z]+>"),
            ("/f/*", "/f/:id"),
            ("/users/{id}", "/users/:id"),
        ] {
            assert_ne!(read(a).0, read(b).0, "{a} / {b}");
        }
    }

    #[test]
    fn the_mount_keeps_the_names_and_drops_the_slip() {
        assert_eq!(read("q/:id/").1, "/q/:id");
        assert_eq!(read("/n/:id<\\d+>/x").1, "/n/:id<\\d+>/x");
        assert_eq!(read("/f/*rest").1, "/f/*rest");
        assert_eq!(read("//").1, "/");
    }

    #[test]
    fn what_poem_refuses_at_boot_is_named() {
        for path in ["/a/:", "/a/:<\\d+>", "/a/<\\d+", "/a/<>"] {
            assert!(RoutePath::parse(path).is_err(), "{path}");
        }
    }
}
