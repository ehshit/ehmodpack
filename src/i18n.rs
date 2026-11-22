use std::cell::RefCell;
use std::env;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use fluent::{FluentArgs, FluentBundle, FluentResource};
use unic_langid::LanguageIdentifier;

const DEFAULT_LOCALE: &str = "en-US";

static CATALOGS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));
static LOCALE: RwLock<Option<String>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static BUNDLE: RefCell<Option<(u64, FluentBundle<FluentResource>)>> = const { RefCell::new(None) };
}

pub fn t(key: &str) -> String {
    t_args(key, &[])
}

pub fn t_args(key: &str, args: &[(&str, &str)]) -> String {
    let generation = GENERATION.load(Ordering::SeqCst);
    BUNDLE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_none_or(|(seen, _)| *seen != generation) {
            *slot = Some((generation, build_bundle()));
        }
        let (_, bundle) = slot.as_ref().expect("bundle must be ready");
        let Some(message) = bundle.get_message(key) else {
            return format!("[{key}]");
        };
        let Some(pattern) = message.value() else {
            return format!("[{key}]");
        };
        let mut fluent_args = FluentArgs::new();
        for (name, value) in args {
            fluent_args.set(*name, *value);
        }
        let mut errors = Vec::new();
        bundle
            .format_pattern(pattern, Some(&fluent_args), &mut errors)
            .into_owned()
    })
}

pub fn bundled_locales() -> Vec<&'static str> {
    CATALOGS.iter().map(|(id, _)| *id).collect()
}

pub fn match_locale(raw: &str) -> Option<&'static str> {
    let locale = normalize_locale(raw)?;
    catalog_for(&locale)
        .or_else(|| {
            let language = locale.split('-').next()?;
            catalog_for_language(language)
        })
        .map(|(id, _)| id)
}

pub fn current_locale() -> &'static str {
    choose_catalog().0
}

pub fn set_locale(raw: &str) -> bool {
    let Some(locale) = match_locale(raw) else {
        return false;
    };
    let mut guard = LOCALE.write().unwrap_or_else(|e| e.into_inner());
    *guard = Some(locale.to_string());
    drop(guard);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    true
}

fn normalize_locale(raw: &str) -> Option<String> {
    let clean = raw.trim().split(['.', '@']).next()?.replace('_', "-");
    if clean.is_empty() || clean.eq_ignore_ascii_case("C") || clean.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    let language = clean.split('-').next()?;
    if language.chars().count() < 2 {
        return None;
    }
    Some(clean)
}

fn push_env_locales(key: &str, out: &mut Vec<String>) {
    let Ok(raw) = env::var(key) else {
        return;
    };
    for part in raw.split(':') {
        if let Some(locale) = normalize_locale(part)
            && !out.contains(&locale)
        {
            out.push(locale);
        }
    }
}

fn requested_locales() -> Vec<String> {
    let mut out = Vec::new();
    push_env_locales("EHMODPACK_LANG", &mut out);
    push_env_locales("LC_ALL", &mut out);
    push_env_locales("LC_MESSAGES", &mut out);
    push_env_locales("LANG", &mut out);
    out
}

fn catalog_for(locale: &str) -> Option<(&'static str, &'static str)> {
    CATALOGS
        .iter()
        .copied()
        .find(|(id, _)| id.eq_ignore_ascii_case(locale))
}

fn catalog_for_language(language: &str) -> Option<(&'static str, &'static str)> {
    CATALOGS.iter().copied().find(|(id, _)| {
        id.split(['-', '_'])
            .next()
            .is_some_and(|head| head.eq_ignore_ascii_case(language))
    })
}

fn choose_catalog() -> (&'static str, &'static str) {
    if let Some(saved) = LOCALE.read().unwrap_or_else(|e| e.into_inner()).as_deref() {
        if let Some(catalog) = catalog_for(saved) {
            return catalog;
        }
    }
    for locale in requested_locales() {
        if let Some(catalog) = catalog_for(&locale) {
            return catalog;
        }
        let language = locale.split('-').next().unwrap_or(&locale);
        if let Some(catalog) = catalog_for_language(language) {
            return catalog;
        }
    }
    catalog_for(DEFAULT_LOCALE).expect("the en-US catalog must be embedded")
}

fn resource(text: &str) -> FluentResource {
    FluentResource::try_new(text.to_string()).expect("embedded FTL catalog must parse")
}

fn build_bundle() -> FluentBundle<FluentResource> {
    let (locale, catalog) = choose_catalog();
    let language = locale
        .parse::<LanguageIdentifier>()
        .unwrap_or_else(|_| DEFAULT_LOCALE.parse().expect("en-US must parse"));
    let mut bundle = FluentBundle::new(vec![language]);
    bundle.set_use_isolating(false);
    let (_, source) = catalog_for(DEFAULT_LOCALE).expect("the en-US catalog must be embedded");
    bundle
        .add_resource(resource(source))
        .expect("the en-US catalog must load");
    if !locale.eq_ignore_ascii_case(DEFAULT_LOCALE) {
        bundle.add_resource_overriding(resource(catalog));
    }
    bundle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_catalog_has_error_prefix() {
        assert_eq!(t("error-prefix"), "Error:");
    }

    #[test]
    fn arguments_are_substituted() {
        assert_eq!(
            t_args(
                "login-go-to",
                &[("url", "https://example.test"), ("code", "ABC123")]
            ),
            "Go to https://example.test and enter code ABC123"
        );
    }

    #[test]
    fn unknown_keys_do_not_panic() {
        assert_eq!(t("definitely-not-a-key"), "[definitely-not-a-key]");
    }
}