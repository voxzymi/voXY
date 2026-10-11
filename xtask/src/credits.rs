//! `cargo xtask credits [--check]`: the README and LICENSING.md credit
//! blocks, generated from `third_party/credits.toml` (design §8).
//!
//! The check also enforces the plugin license policy on the data itself:
//! no copyleft license (GPL, LGPL, AGPL) may appear in any entry, and every
//! entry needs its pinned commit, license link and copyright line.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema: u32,
    #[serde(default)]
    credit: Vec<Credit>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credit {
    id: String,
    name: String,
    authors: Vec<String>,
    url: String,
    version: Option<String>,
    commit: String,
    license: String,
    license_url: String,
    copyright: String,
    #[serde(rename = "use")]
    usage: Usage,
    status: Status,
    note: Option<String>,
    sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Usage {
    HostApi,
    Fixture,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Approved,
    Proposed,
}

/// `third_party/ofx-fixtures.toml`: the plugins CI builds as fixtures. It
/// arrives with the fixture builds; until then there is nothing to check.
#[derive(Debug, Deserialize)]
struct Fixtures {
    #[serde(default)]
    fixture: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    credit: String,
}

/// Licenses a credited project may have: permissive ones only.
const ALLOWED_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "BSL-1.0",
    "0BSD",
    "Unlicense",
];

const BEGIN: &str =
    "<!-- credits:begin (generated from third_party/credits.toml; do not edit by hand) -->";
const END: &str = "<!-- credits:end -->";

pub fn run(root: &Path, check: bool) -> Result<()> {
    let text = std::fs::read_to_string(root.join("third_party/credits.toml"))?;
    let file: File = toml::from_str(&text).context("third_party/credits.toml")?;
    validate(&file)?;
    check_fixtures(root, &file)?;
    let approved: Vec<&Credit> = file
        .credit
        .iter()
        .filter(|c| c.status == Status::Approved)
        .collect();
    let mut stale = Vec::new();
    for (name, block) in [
        ("README.md", readme(&approved)),
        ("LICENSING.md", licensing(&approved)),
    ] {
        let path = root.join(name);
        let current = std::fs::read_to_string(&path)?.replace("\r\n", "\n");
        let updated = splice(&current, &block).with_context(|| name.to_owned())?;
        if updated != current {
            if check {
                stale.push(name);
            } else {
                std::fs::write(&path, updated)?;
                println!("updated {name}");
            }
        }
    }
    if !stale.is_empty() {
        bail!(
            "credit blocks out of date in {}: run `cargo xtask credits`",
            stale.join(", ")
        );
    }
    if check {
        println!("credit blocks are up to date");
    }
    Ok(())
}

fn validate(file: &File) -> Result<()> {
    if file.schema != 1 {
        bail!("credits.toml: unknown schema {}", file.schema);
    }
    let mut ids = std::collections::HashSet::new();
    for c in &file.credit {
        let id = &c.id;
        if !ids.insert(id.as_str()) {
            bail!("credits.toml: duplicate id {id}");
        }
        if c.license.to_ascii_uppercase().contains("GPL") {
            bail!(
                "credits.toml: {id} is copyleft ({}); voXY uses no GPL, LGPL or AGPL plugins",
                c.license
            );
        }
        if !ALLOWED_LICENSES.contains(&c.license.as_str()) {
            bail!(
                "credits.toml: {id} has license {}, which is not on the permissive list",
                c.license
            );
        }
        if c.commit.len() != 40 || !c.commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("credits.toml: {id} must pin a full 40-character commit");
        }
        for (field, v) in [
            ("name", &c.name),
            ("url", &c.url),
            ("license_url", &c.license_url),
            ("copyright", &c.copyright),
        ] {
            if v.trim().is_empty() {
                bail!("credits.toml: {id} has an empty {field}");
            }
        }
        if c.authors.is_empty() {
            bail!("credits.toml: {id} lists no authors");
        }
        if c.usage == Usage::Download && c.status == Status::Approved && c.sha256.is_none() {
            bail!("credits.toml: {id} is offered for download without a sha256");
        }
    }
    Ok(())
}

fn check_fixtures(root: &Path, file: &File) -> Result<()> {
    let path = root.join("third_party/ofx-fixtures.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let fixtures: Fixtures = toml::from_str(&text).context("third_party/ofx-fixtures.toml")?;
    for f in &fixtures.fixture {
        let ok = file
            .credit
            .iter()
            .any(|c| c.id == f.credit && c.usage == Usage::Fixture && c.status == Status::Approved);
        if !ok {
            bail!(
                "ofx-fixtures.toml: {} is not an approved fixture in credits.toml",
                f.credit
            );
        }
    }
    Ok(())
}

/// Replaces the text between the markers (the markers stay).
fn splice(doc: &str, block: &str) -> Result<String> {
    let start = doc.find(BEGIN).context("no credits:begin marker")? + BEGIN.len();
    let end = doc[start..].find(END).context("no credits:end marker")? + start;
    Ok(format!("{}\n{block}{}", &doc[..start], &doc[end..]))
}

fn used_for(c: &Credit) -> String {
    match c.usage {
        Usage::HostApi => c
            .note
            .clone()
            .unwrap_or_else(|| "voXY implements this API.".to_owned()),
        Usage::Fixture => "CI test fixtures only; not shipped.".to_owned(),
        Usage::Download => "Offered as an optional download.".to_owned(),
    }
}

fn downloads_line(credits: &[&Credit]) -> &'static str {
    if credits.iter().any(|c| c.usage == Usage::Download) {
        "Plugins marked as an optional download are fetched only when the user asks."
    } else {
        "voXY offers no plugins for download."
    }
}

fn readme(credits: &[&Credit]) -> String {
    let mut s = String::from(
        "voXY hosts OpenFX image-effect plugins. The projects below are credited\n\
         because voXY implements their API or uses their plugins. **They are not\n\
         affiliated with or endorsed by voXY. voXY only hosts them.**\n\n\
         | Project | Authors | License | Used for |\n|---|---|---|---|\n",
    );
    for c in credits {
        let version = c
            .version
            .as_ref()
            .map(|v| format!(" ({v})"))
            .unwrap_or_default();
        let _ = writeln!(
            s,
            "| [{}]({}){version} | {} | {} | {} |",
            c.name,
            c.url,
            c.authors.join(", "),
            c.license,
            used_for(c)
        );
    }
    let _ = write!(
        s,
        "\n{} Details, and the plugins that were\nconsidered, are in \
         [`docs/ofx-plugins.md`](docs/ofx-plugins.md).\n",
        downloads_line(credits)
    );
    s
}

fn licensing(credits: &[&Credit]) -> String {
    let mut s = String::new();
    for c in credits {
        let tag = c
            .version
            .as_ref()
            .map(|v| format!("tag `{v}`, "))
            .unwrap_or_default();
        let usage = match c.usage {
            Usage::HostApi => {
                "voXY's host implements this API. The headers are vendored in\n\
                 `third_party/openfx/` with this license file."
            }
            Usage::Fixture => {
                "built from source in CI as test fixtures only. They are never\n\
                 shipped, uploaded or linked into voXY, so no binary-distribution\n\
                 notice obligation arises."
            }
            Usage::Download => {
                "offered as an optional download, fetched only when the user asks\n\
                 and checked against its pinned sha256."
            }
        };
        let _ = write!(
            s,
            "**{}**: <{}>,\n{tag}commit `{}`.\nLicense: {} ([full text]({})),\n\"{}\". Use:\n{usage}\n\n",
            c.name, c.url, c.commit, c.license, c.license_url, c.copyright
        );
    }
    let _ = writeln!(
        s,
        "voXY uses no copyleft (GPL, LGPL, AGPL) plugins anywhere. {}",
        downloads_line(credits)
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &str) -> Result<File> {
        let text = format!(
            "schema = 1\n[[credit]]\nid = \"x\"\nname = \"X\"\nauthors = [\"A\"]\n\
             url = \"https://example.org\"\ncommit = \"{}\"\nlicense_url = \"https://example.org/L\"\n\
             copyright = \"Copyright A\"\nuse = \"fixture\"\nstatus = \"approved\"\n{extra}",
            "0".repeat(40)
        );
        Ok(toml::from_str(&text)?)
    }

    #[test]
    fn copyleft_and_unknown_licenses_are_refused() {
        for bad in [
            "GPL-2.0-or-later",
            "LGPL-2.1-only",
            "AGPL-3.0",
            "CC-BY-NC-4.0",
        ] {
            let f = parse(&format!("license = \"{bad}\"")).unwrap();
            assert!(validate(&f).is_err(), "{bad}");
        }
        let f = parse("license = \"BSD-3-Clause\"").unwrap();
        assert!(validate(&f).is_ok());
    }

    #[test]
    fn splice_keeps_the_markers_and_the_rest() {
        let doc = format!("a\n{BEGIN}\nold\n{END}\nb\n");
        assert_eq!(
            splice(&doc, "new\n").unwrap(),
            format!("a\n{BEGIN}\nnew\n{END}\nb\n")
        );
    }
}
