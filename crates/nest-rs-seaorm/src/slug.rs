//! Slug generation and collision-free allocation for soft-deletable entities.
//!
//! [`resolve_unique_slug`] is the public entry point: it slugifies a source
//! string and walks suffixes until it finds one no live row holds, scoped by an
//! optional extra [`Condition`] (e.g. a tenant column). The text helpers
//! (`slugify`, `with_suffix`) are private — the only caller is the resolver.

use std::borrow::Cow;

use sea_orm::sea_query::Condition;
use sea_orm::{ColumnTrait, ConnectionTrait, QueryFilter};

use crate::{ServiceError, SoftDeletable, live_condition};

/// How many `base`, `base-2`, `base-3`, … candidates to try before giving up.
const MAX_ATTEMPTS: u32 = 100;

/// Allocate a slug unique among the **live** rows of `E`, optionally within a
/// scope. Slugifies `source`; falls back to `fallback` when slugification yields
/// an empty string (e.g. a source of only punctuation). `extra` ANDs onto every
/// lookup — pass [`Condition::all`]`()` for a globally-unique slug, or a tenant
/// predicate (`Column::OrgId.eq(id)`) for per-scope uniqueness.
///
/// Returns the first free candidate (`base`, then `base-2`, `base-3`, …), or a
/// [`ServiceError`] after [`MAX_ATTEMPTS`] collisions.
///
/// Queries the connection directly (not [`Repo`](crate::Repo)) so the probe is
/// **unscoped by ability**: a slug must be unique across every live row,
/// including ones the caller cannot see. Per-tenant uniqueness is opted into
/// explicitly via `extra`, never inferred from the ambient ability.
pub async fn resolve_unique_slug<E, C>(
    conn: &C,
    slug_column: E::Column,
    source: &str,
    fallback: &str,
    extra: Condition,
) -> Result<String, ServiceError>
where
    E: SoftDeletable,
    C: ConnectionTrait,
{
    let slug = slugify(source);
    let base: &str = if slug.is_empty() { fallback } else { &slug };

    for attempt in 1..=MAX_ATTEMPTS {
        let candidate = with_suffix(base, attempt);
        let taken = E::find()
            .filter(live_condition::<E>())
            .filter(extra.clone())
            .filter(slug_column.eq(candidate.clone()))
            .one(conn)
            .await?;
        if taken.is_none() {
            return Ok(candidate);
        }
    }

    Err(ServiceError::internal(format!(
        "could not allocate unique {fallback} slug"
    )))
}

/// Lowercase ASCII slug: transliterate, keep alphanumerics, collapse every other
/// run to a single dash, trim leading/trailing dashes.
fn slugify(input: &str) -> String {
    let normalized = transliterate(input);
    let mut slug = String::new();
    let mut last_dash = false;

    for ch in normalized.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

/// `base` for the first attempt, `base-N` afterwards.
fn with_suffix(base: &str, attempt: u32) -> String {
    if attempt <= 1 {
        base.to_string()
    } else {
        format!("{base}-{attempt}")
    }
}

/// Map the Latin-1 accented letters to their ASCII base. Non-Latin scripts pass
/// through unchanged (and are then dropped by [`slugify`]) — a documented limit;
/// reach for a full transliteration crate (`deunicode`) if that becomes a need.
fn transliterate(input: &str) -> Cow<'_, str> {
    if input.is_ascii() {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        out.push(match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'æ' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
            'Ç' => 'c',
            'È' | 'É' | 'Ê' | 'Ë' => 'e',
            'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
            'Ñ' => 'n',
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'o',
            'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
            'Ý' => 'y',
            other => other,
        });
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Galerie Metropole"), "galerie-metropole");
    }

    #[test]
    fn slugify_transliterates() {
        assert_eq!(slugify("Café Müller"), "cafe-muller");
    }

    #[test]
    fn with_suffix_appends_number() {
        assert_eq!(with_suffix("galerie", 3), "galerie-3");
    }
}
