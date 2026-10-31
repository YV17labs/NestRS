//! Test doubles: an environment holding a fixed set of variables.

/// The variables `set` holds, and nothing else.
pub(crate) fn reading(set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let set: Vec<(String, String)> = set
        .iter()
        .map(|(variable, value)| ((*variable).to_owned(), (*value).to_owned()))
        .collect();
    move |name: &str| {
        set.iter()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.clone())
    }
}
