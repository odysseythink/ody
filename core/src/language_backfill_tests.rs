use super::*;
use ody_config::CONFIG_TOML_FILE;
use pretty_assertions::assert_eq;

fn config_toml(language: Option<&str>) -> ConfigToml {
    let body = match language {
        Some(language) => format!("language = \"{language}\"\n"),
        None => String::new(),
    };
    toml::from_str(&body).expect("valid toml")
}

fn read_config(ody_home: &Path) -> Option<String> {
    std::fs::read_to_string(ody_home.join(CONFIG_TOML_FILE)).ok()
}

#[tokio::test]
async fn skips_when_language_is_explicit() {
    let home = tempfile::tempdir().unwrap();
    let status = backfill_language_if_needed(home.path(), &config_toml(Some("en")))
        .await
        .unwrap();
    assert_eq!(status, LanguageBackfillStatus::SkippedExplicit);
    // Nothing should be written when the field is already set.
    assert_eq!(read_config(home.path()), None);
}

#[tokio::test]
async fn skips_when_language_is_the_auto_sentinel() {
    // `auto` is an explicit opt-in to per-run detection and must be respected.
    let home = tempfile::tempdir().unwrap();
    let status = backfill_language_if_needed(home.path(), &config_toml(Some("auto")))
        .await
        .unwrap();
    assert_eq!(status, LanguageBackfillStatus::SkippedExplicit);
    assert_eq!(read_config(home.path()), None);
}

#[tokio::test]
async fn backfills_when_language_is_absent() {
    let home = tempfile::tempdir().unwrap();
    let status = backfill_language_if_needed(home.path(), &config_toml(None))
        .await
        .unwrap();

    // Detection is environment-dependent: assert the outcome matches whatever
    // the system locale resolves to, and that a resolved code is persisted.
    match ody_config::locale::detect_system_locale_code() {
        Some(code) => {
            assert_eq!(status, LanguageBackfillStatus::Applied(code.clone()));
            let written = read_config(home.path()).expect("config.toml written");
            assert!(
                written.contains(&format!("language = \"{code}\"")),
                "expected persisted language code, got: {written}"
            );
        }
        None => {
            assert_eq!(status, LanguageBackfillStatus::SkippedUndetected);
            assert_eq!(read_config(home.path()), None);
        }
    }
}

#[tokio::test]
async fn backfills_when_language_is_empty_string() {
    let home = tempfile::tempdir().unwrap();
    let status = backfill_language_if_needed(home.path(), &config_toml(Some("   ")))
        .await
        .unwrap();
    match ody_config::locale::detect_system_locale_code() {
        Some(code) => assert_eq!(status, LanguageBackfillStatus::Applied(code)),
        None => assert_eq!(status, LanguageBackfillStatus::SkippedUndetected),
    }
}
