use std::process;

use anyhow::{Context, Result, anyhow};
use forager_sdk::Forager;
use schemars::JsonSchema;
use serde::Deserialize;
use wezel_types::ForagerPluginOutput;

#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum PackageSpecifier {
    Packages(Vec<String>),
    Workspace(WorkspaceTag),
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspaceTag {
    Workspace,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Command {
    #[default]
    Build,
    Bench,
    Test,
}

impl Command {
    fn as_str(&self) -> &'static str {
        match self {
            Command::Build => "build",
            Command::Bench => "bench",
            Command::Test => "test",
        }
    }
}

#[derive(Deserialize, JsonSchema)]
struct CargoInputs {
    /// Cargo subcommand to invoke (`build`, `bench`, or `test`).
    command: Command,
    /// Build target — either a list of package names or the literal string "workspace".
    build_target: PackageSpecifier,
}

struct Cargo;

impl Forager for Cargo {
    const NAME: &'static str = "cargo";
    const DESCRIPTION: &'static str = "Runs `cargo <command>` and records wall-clock time";
    const MEASUREMENTS_DOC: &'static str =
        "**`time_ms`** — how long `cargo <command>` took, in milliseconds.";
    type Inputs = CargoInputs;

    fn run(inputs: CargoInputs) -> Result<Vec<ForagerPluginOutput>> {
        let mut child = process::Command::new("cargo");
        child.arg(inputs.command.as_str());
        match inputs.build_target {
            PackageSpecifier::Packages(items) => {
                items
                    .into_iter()
                    .fold(&mut child, |child, package| child.arg("-p").arg(package));
            }
            PackageSpecifier::Workspace(WorkspaceTag::Workspace) => {
                child.arg("--workspace");
            }
        }

        let timer_start = std::time::Instant::now();
        let status = child.status().context("failed to spawn cargo")?;
        let elapsed = timer_start.elapsed();

        if !status.success() {
            return Err(anyhow!("cargo exited with {status}"));
        }

        let ms: u64 = elapsed
            .as_millis()
            .try_into()
            .context("build duration overflowed u64")?;
        Ok(vec![ForagerPluginOutput {
            name: "time_ms".to_owned(),
            value: ms.into(),
            tags: Default::default(),
        }])
    }
}

forager_sdk::forager_main!(Cargo);

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> CargoInputs {
        serde_json::from_str(s).expect("should parse")
    }

    #[test]
    fn workspace_target_from_string() {
        let inputs = parse(r#"{"command":"build","build_target":"workspace"}"#);
        assert!(matches!(inputs.command, Command::Build));
        assert!(matches!(
            inputs.build_target,
            PackageSpecifier::Workspace(WorkspaceTag::Workspace)
        ));
    }

    #[test]
    fn packages_target_from_array() {
        let inputs = parse(r#"{"command":"test","build_target":["forager_cargo","wezel_types"]}"#);
        assert!(matches!(inputs.command, Command::Test));
        match inputs.build_target {
            PackageSpecifier::Packages(items) => {
                assert_eq!(items, vec!["forager_cargo", "wezel_types"]);
            }
            _ => panic!("expected Packages variant"),
        }
    }

    #[test]
    fn command_is_lowercase() {
        assert!(
            serde_json::from_str::<CargoInputs>(
                r#"{"command":"Build","build_target":"workspace"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn workspace_tag_is_lowercase() {
        assert!(
            serde_json::from_str::<CargoInputs>(
                r#"{"command":"build","build_target":"WORKSPACE"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn null_build_target_rejected() {
        assert!(
            serde_json::from_str::<CargoInputs>(r#"{"command":"build","build_target":null}"#)
                .is_err()
        );
    }

    #[test]
    fn missing_build_target_rejected() {
        assert!(serde_json::from_str::<CargoInputs>(r#"{"command":"build"}"#).is_err());
    }
}
