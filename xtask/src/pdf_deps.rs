//! Pin the shared parser and reject interpreter features that would embed backend assets.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const GIT: &str = "https://github.com/storytold/craft-libs";
const REV: &str = "92d392cff132eaf53e13afff77f38b5c11b73aa0";
const PACKAGES: &[(&str, &str)] = &[("craft-pdf", "0.1.0"), ("hayro-syntax", "0.7.2"), ("hayro-jbig2", "0.3.0")];

/// Checks that the parser resolves once, to craft-libs at its pinned commit, with only the read
/// features. A build of a fork of craft-libs names the fork with `CRAFT_LIBS_GIT` and
/// `CRAFT_LIBS_REV` (see `source`).
pub fn run(root: &Path) -> Result<(), String> {
    let (source, forked) = source_from_env(|name| std::env::var_os(name))?;
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(["metadata", "--locked", "--format-version", "1"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    validate(&metadata, &source)?;
    if forked {
        println!("craft-libs: {source}, named by CRAFT_LIBS_GIT and CRAFT_LIBS_REV");
    }
    println!("PDF backend: pinned parser only; no Hayro interpreter, rasterizer or bundled CMaps");
    Ok(())
}

/// The source `run` checks against, read from the build environment through `var`, and whether
/// `CRAFT_LIBS_GIT` and `CRAFT_LIBS_REV` named it. A variable that is set counts, even when it is
/// empty, so a mistake is reported rather than ignored.
fn source_from_env(var: impl Fn(&str) -> Option<OsString>) -> Result<(String, bool), String> {
    let text = |name: &str| var(name).map(|v| v.into_string().map_err(|_| format!("{name} is not valid Unicode"))).transpose();
    let (git, rev) = (text("CRAFT_LIBS_GIT")?, text("CRAFT_LIBS_REV")?);
    let forked = git.is_some() || rev.is_some();
    Ok((source(git, rev)?, forked))
}

/// The craft-libs source the parser must resolve to, as Cargo records it: upstream's pin, or a
/// fork's. A fork is named by both `CRAFT_LIBS_GIT`, the URL as Cargo.lock records it, and
/// `CRAFT_LIBS_REV`, the full commit hash, so that a branch or tag can never take its place.
fn source(git: Option<String>, rev: Option<String>) -> Result<String, String> {
    let (git, rev) = match (git, rev) {
        (None, None) => (GIT.to_owned(), REV.to_owned()),
        (Some(git), Some(rev)) => (git, rev),
        _ => return Err("set both CRAFT_LIBS_GIT and CRAFT_LIBS_REV, or neither".into()),
    };
    check_url(&git)?;
    if rev.len() != 40 || !rev.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(format!("CRAFT_LIBS_REV must be a full 40-digit lowercase commit hash, as Cargo.lock records it, not {rev:?}"));
    }
    Ok(format!("git+{git}?rev={rev}#{rev}"))
}

/// Refuses a `CRAFT_LIBS_GIT` that isn't a `scheme://` URL, as Cargo needs, or that carries
/// credentials: only an `ssh://` URL may name a user, and never with a password, since an
/// `https://` user name is often a token. The error never repeats the URL.
fn check_url(git: &str) -> Result<(), String> {
    let Some((scheme, rest)) = git.split_once("://") else {
        return Err("CRAFT_LIBS_GIT must be a URL such as https://host/craft-libs or ssh://git@host/craft-libs".into());
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    match authority.rsplit_once('@') {
        None => Ok(()),
        Some((user, _)) if scheme.eq_ignore_ascii_case("ssh") && !user.contains(':') => Ok(()),
        Some(_) => {
            Err("CRAFT_LIBS_GIT must not carry credentials: use a credential helper or an SSH key (an ssh:// URL may name a user, without a password)".into())
        }
    }
}

/// Checks the resolved graph: each parser package once, at its version and from `source`, with
/// only its allowed feature, and none of the interpreter's packages active.
fn validate(metadata: &Value, source: &str) -> Result<(), String> {
    let packages = metadata.get("packages").and_then(Value::as_array).ok_or("cargo metadata has no packages")?;
    let nodes = metadata.pointer("/resolve/nodes").and_then(Value::as_array).ok_or("cargo metadata has no resolved nodes")?;
    for (name, version) in PACKAGES {
        let matches: Vec<_> = packages.iter().filter(|p| p["name"] == *name).collect();
        if matches.len() != 1 || matches[0]["version"] != *version || matches[0]["source"] != source {
            return Err(format!("{name}: expected exactly {version} from {source}"));
        }
    }
    for (name, allowed) in [("craft-pdf", "read"), ("hayro-syntax", "std")] {
        let package = packages.iter().find(|p| p["name"] == name).ok_or_else(|| format!("missing {name}"))?;
        let node = nodes.iter().find(|n| n["id"] == package["id"]).ok_or_else(|| format!("{name}: missing resolved node"))?;
        let features = node["features"].as_array().ok_or_else(|| format!("{name}: missing resolved features"))?;
        if features.len() != 1 || features.first().and_then(Value::as_str) != Some(allowed) {
            return Err(format!("{name}: expected only the {allowed} feature"));
        }
    }
    for package in packages {
        let name = package["name"].as_str().unwrap_or("");
        let active = nodes.iter().any(|node| node["id"] == package["id"]);
        if active && matches!(name, "hayro" | "hayro-interpret" | "hayro-cmap" | "krilla") {
            return Err(format!("{name}: EffectCraft must use craft-pdf with only the read feature"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Value {
        let packages: Vec<_> = PACKAGES.iter().map(|(name, version)| {
            json!({"id": name, "name": name, "version": version, "source": format!("git+https://github.com/storytold/craft-libs?rev={REV}#{REV}")})
        }).collect();
        json!({"packages": packages, "resolve": {"nodes": [
            {"id": "hayro-syntax", "features": ["std"]}, {"id": "craft-pdf", "features": ["read"]}
        ]}})
    }

    const FORK_GIT: &str = "https://example.org/craft-libs";
    const FORK_REV: &str = "ba45867c71639154c5f2b03ff609e34bd0da149e";

    /// The check as upstream runs it, with neither variable set.
    fn validate(metadata: &Value) -> Result<(), String> {
        super::validate(metadata, &source(None, None).unwrap())
    }

    fn fork(rev: &str) -> Result<String, String> {
        source(Some(FORK_GIT.into()), Some(rev.into()))
    }

    #[test]
    fn accepts_a_fork_of_craft_libs_only_where_it_is_named() {
        let fork = fork(FORK_REV).unwrap();
        let mut m = metadata();
        for p in m["packages"].as_array_mut().unwrap() {
            p["source"] = json!(fork);
        }
        assert!(super::validate(&m, &fork).is_ok());
        assert!(validate(&m).is_err());
        assert!(super::validate(&metadata(), &fork).is_err());
    }

    #[test]
    fn naming_upstream_is_the_same_as_naming_nothing() {
        assert_eq!(source(None, None).unwrap(), format!("git+https://github.com/storytold/craft-libs?rev={REV}#{REV}"));
        assert_eq!(source(Some(GIT.into()), Some(REV.into())), source(None, None));
    }

    #[test]
    fn rejects_half_a_fork_moving_or_short_revisions_and_credentials() {
        assert!(source(Some(FORK_GIT.into()), None).is_err());
        assert!(source(None, Some(FORK_REV.into())).is_err());
        let (upper, long) = (FORK_REV.to_uppercase(), format!("{FORK_REV}0"));
        for rev in ["main", "v1.0.0", "ba45867", upper.as_str(), long.as_str()] {
            assert!(fork(rev).is_err(), "{rev}");
        }
        for git in [
            "https://name:secret@example.org/craft-libs",
            "https://secret@example.org/craft-libs",
            "ssh://git:secret@example.org/craft-libs",
            "secret@example.org:craft-libs",
            "craft-libs",
            "",
        ] {
            let error = source(Some(git.into()), Some(FORK_REV.into())).unwrap_err();
            assert!(!error.contains("secret") && !error.contains("example"), "{git}: {error}");
        }
    }

    #[test]
    fn accepts_a_user_name_only_in_an_ssh_url() {
        for git in ["ssh://git@example.org/craft-libs", "ssh://deploy@example.org/craft-libs", "https://example.org/a@b/craft-libs"] {
            assert_eq!(source(Some(git.into()), Some(FORK_REV.into())), Ok(format!("git+{git}?rev={FORK_REV}#{FORK_REV}")));
        }
    }

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn reads_the_fork_from_the_environment_as_run_does() {
        let both = [("CRAFT_LIBS_GIT", FORK_GIT), ("CRAFT_LIBS_REV", FORK_REV)];
        assert_eq!(source_from_env(env(&both)), Ok((fork(FORK_REV).unwrap(), true)));
        assert_eq!(source_from_env(env(&[])), Ok((source(None, None).unwrap(), false)));
        for set in [&[("CRAFT_LIBS_GIT", ""), ("CRAFT_LIBS_REV", "")][..], &[("CRAFT_LIBS_GIT", FORK_GIT)], &[("CRAFT_LIBS_REV", "")]] {
            assert!(source_from_env(env(set)).is_err(), "{set:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_variable_that_is_not_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let var = |name: &str| (name == "CRAFT_LIBS_GIT").then(|| OsString::from_vec(vec![0xff]));
        assert_eq!(source_from_env(var), Err("CRAFT_LIBS_GIT is not valid Unicode".into()));
    }

    #[test]
    fn accepts_the_pinned_parser_only_graph() {
        assert!(validate(&metadata()).is_ok());
    }

    #[test]
    fn rejects_unpinned_duplicate_and_missing_parsers() {
        let mut m = metadata();
        m["packages"][0]["source"] = json!("registry");
        assert!(validate(&m).is_err());
        let mut m = metadata();
        let duplicate = m["packages"][0].clone();
        m["packages"].as_array_mut().unwrap().push(duplicate);
        assert!(validate(&m).is_err());
        m["packages"].as_array_mut().unwrap().retain(|p| p["name"] != "hayro-syntax");
        assert!(validate(&m).is_err());
    }

    #[test]
    fn rejects_shared_reader_source_drift() {
        let mut m = metadata();
        m["packages"][0]["source"] = Value::Null;
        assert!(validate(&m).is_err());
    }

    #[test]
    fn rejects_extra_features_and_missing_active_nodes() {
        for (index, feature) in [(0, "images"), (1, "write"), (1, "raster")] {
            let mut m = metadata();
            m["resolve"]["nodes"][index]["features"].as_array_mut().unwrap().push(json!(feature));
            assert!(validate(&m).is_err());
        }
        let mut m = metadata();
        m["resolve"]["nodes"].as_array_mut().unwrap().pop();
        assert!(validate(&m).is_err());
    }

    #[test]
    fn rejects_backend_asset_features_and_missing_resolution() {
        for name in ["hayro", "hayro-interpret", "hayro-cmap", "krilla"] {
            let mut m = metadata();
            m["packages"].as_array_mut().unwrap().push(json!({"id": name, "name": name}));
            m["resolve"]["nodes"].as_array_mut().unwrap().push(json!({"id": name}));
            assert!(validate(&m).is_err());
        }
        let mut m = metadata();
        m["resolve"] = Value::Null;
        assert!(validate(&m).is_err());
    }
}
