//! The language of voice calls and dictation (docs/plan/voice-call.md § Language): the user's
//! choice in Settings, or the device's own language. Pure: the platform's language is read by
//! the window (`yaac-app`) or the renderer (Android) and handed in.

/// The languages Settings offers, as BCP-47 tags with their own names. Any other tag a
/// device reports is still used as its default.
pub const CHOICES: &[(&str, &str)] = &[
    ("en-US", "English (United States)"),
    ("en-GB", "English (United Kingdom)"),
    ("pt-BR", "Português (Brasil)"),
    ("pt-PT", "Português (Portugal)"),
    ("es-ES", "Español (España)"),
    ("es-419", "Español (Latinoamérica)"),
    ("fr-FR", "Français"),
    ("de-DE", "Deutsch"),
    ("it-IT", "Italiano"),
    ("nl-NL", "Nederlands"),
    ("pl-PL", "Polski"),
    ("ru-RU", "Русский"),
    ("uk-UA", "Українська"),
    ("tr-TR", "Türkçe"),
    ("ar", "العربية"),
    ("hi-IN", "हिन्दी"),
    ("ja-JP", "日本語"),
    ("ko-KR", "한국어"),
    ("zh-CN", "中文（简体）"),
    ("zh-TW", "中文（繁體）"),
];

/// What Settings shows for "no choice": the device's language is used.
pub const SAME_AS_DEVICE: &str = "Same as this device";

/// The label of a stored choice: `""` is [`SAME_AS_DEVICE`], a listed tag its name, any other
/// tag itself.
pub fn label(choice: &str) -> &str {
    if choice.is_empty() {
        return SAME_AS_DEVICE;
    }
    CHOICES.iter().find(|(tag, _)| *tag == choice).map_or(choice, |(_, name)| name)
}

/// `tag` in its canonical case (`pt-br` → `pt-BR`, `zh-hans-cn` → `zh-Hans-CN`) when it is a
/// BCP-47 tag YAAC sends: a 2–3 letter language, optional script, region and variants, at most
/// 35 characters. Anything else (`C`, `POSIX`, a name) is `None`.
pub fn normalize(tag: &str) -> Option<String> {
    let tag = tag.trim();
    if tag.is_empty() || tag.len() > 35 {
        return None;
    }
    let mut out = Vec::new();
    for (i, part) in tag.split('-').enumerate() {
        if part.is_empty() || part.len() > 8 || !part.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let p = if i == 0 {
            if !(2..=3).contains(&part.len()) || !part.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            part.to_ascii_lowercase()
        } else if part.len() == 4 && part.chars().all(|c| c.is_ascii_alphabetic()) {
            let mut s = part.to_ascii_lowercase();
            s[..1].make_ascii_uppercase();
            s
        } else if part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()) {
            part.to_ascii_uppercase()
        } else {
            part.to_ascii_lowercase()
        };
        out.push(p);
    }
    Some(out.join("-"))
}

/// A POSIX locale (`pt_BR.UTF-8`, `de_DE@euro`, `en`) as a BCP-47 tag; `C` and `POSIX` are none.
pub fn from_posix(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next().unwrap_or_default();
    if base.eq_ignore_ascii_case("C") || base.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    normalize(&base.replace('_', "-"))
}

/// The language to send: the user's stored choice, else the device's, else none (the
/// harness's own default).
pub fn effective(choice: &str, device: Option<&str>) -> Option<String> {
    normalize(choice).or_else(|| device.and_then(normalize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_normalized_and_junk_is_refused() {
        assert_eq!(normalize("pt-br").as_deref(), Some("pt-BR"));
        assert_eq!(normalize("ZH-hans-cn").as_deref(), Some("zh-Hans-CN"));
        assert_eq!(normalize("es-419").as_deref(), Some("es-419"));
        for bad in ["", "p", "pt_BR", "Portuguese", "pt-BR; speak English", "en--US"] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
    }

    #[test]
    fn posix_locales_become_bcp47() {
        assert_eq!(from_posix("pt_BR.UTF-8").as_deref(), Some("pt-BR"));
        assert_eq!(from_posix("de_DE@euro").as_deref(), Some("de-DE"));
        assert_eq!(from_posix("en").as_deref(), Some("en"));
        assert_eq!(from_posix("C.UTF-8"), None);
        assert_eq!(from_posix("POSIX"), None);
    }

    #[test]
    fn the_choice_wins_over_the_device_and_blank_means_the_device() {
        assert_eq!(effective("", Some("pt-BR")).as_deref(), Some("pt-BR"));
        assert_eq!(effective("fr-FR", Some("pt-BR")).as_deref(), Some("fr-FR"));
        assert_eq!(effective("", None), None);
        assert_eq!(label(""), SAME_AS_DEVICE);
        assert_eq!(label("pt-BR"), "Português (Brasil)");
        assert_eq!(label("eo"), "eo");
        assert!(CHOICES.iter().all(|(t, _)| normalize(t).as_deref() == Some(*t)), "listed tags are canonical");
    }
}
