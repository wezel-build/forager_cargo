use std::process;

use anyhow::{Context, Result, anyhow};
use forager_sdk::{Forager, ForagerPluginOutput};
use schemars::JsonSchema;
use serde::Deserialize;

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

/// Selects which targets of a given kind to build — either the literal string
/// "all" or a list of target names.
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum TargetSpecifier {
    Names(Vec<String>),
    All(AllTag),
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum AllTag {
    All,
}

impl TargetSpecifier {
    /// Appends the appropriate cargo flags to `child`. `all_flag` is the plural
    /// flag that selects every target of the kind (e.g. `--examples`), and
    /// `one_flag` is the singular flag that selects one by name (e.g. `--example`).
    fn append(&self, child: &mut process::Command, all_flag: &str, one_flag: &str) {
        match self {
            TargetSpecifier::All(AllTag::All) => {
                child.arg(all_flag);
            }
            TargetSpecifier::Names(names) => {
                for name in names {
                    child.arg(one_flag).arg(name);
                }
            }
        }
    }
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
    /// Cargo profile to use, propagated as `--profile <value>`. Omit to use the default profile.
    #[serde(default)]
    profile: Option<String>,
    /// Build every target (lib, bins, tests, benches, examples) via `--all-targets`.
    #[serde(default)]
    all_targets: bool,
    /// Build the library target via `--lib`.
    #[serde(default)]
    lib: bool,
    /// Examples to build, as `--examples` (all) or `--example <name>` per name. Omit to build none.
    #[serde(default)]
    examples: Option<TargetSpecifier>,
    /// Benchmarks to build, as `--benches` (all) or `--bench <name>` per name. Omit to build none.
    #[serde(default)]
    benches: Option<TargetSpecifier>,
    /// Test targets to build, as `--tests` (all) or `--test <name>` per name. Omit to build none.
    #[serde(default)]
    tests: Option<TargetSpecifier>,
    /// Binaries to build, as `--bins` (all) or `--bin <name>` per name. Omit to build none.
    #[serde(default)]
    bins: Option<TargetSpecifier>,
}

/// Renders a spawned command back into a readable invocation string for error
/// messages, e.g. `cargo build --workspace --example foo`.
fn render_command(cmd: &process::Command) -> String {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

struct Cargo;

impl Forager for Cargo {
    const NAME: &'static str = "cargo";
    const DESCRIPTION: &'static str = "Runs `cargo <command>` and records wall-clock time";
    const OUTCOMES_DOC: &'static str =
        "**`time_ms`** — how long `cargo <command>` took, in milliseconds.";
    type Inputs = CargoInputs;

    fn run(inputs: CargoInputs) -> Result<Vec<ForagerPluginOutput>> {
        let mut child = process::Command::new("cargo");
        child.arg(inputs.command.as_str());
        if let Some(profile) = inputs.profile.as_deref() {
            child.arg("--profile").arg(profile);
        }
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

        if inputs.all_targets {
            child.arg("--all-targets");
        }
        if inputs.lib {
            child.arg("--lib");
        }
        if let Some(examples) = &inputs.examples {
            examples.append(&mut child, "--examples", "--example");
        }
        if let Some(benches) = &inputs.benches {
            benches.append(&mut child, "--benches", "--bench");
        }
        if let Some(tests) = &inputs.tests {
            tests.append(&mut child, "--tests", "--test");
        }
        if let Some(bins) = &inputs.bins {
            bins.append(&mut child, "--bins", "--bin");
        }

        let timer_start = std::time::Instant::now();
        let status = child.status().context("failed to spawn cargo")?;
        let elapsed = timer_start.elapsed();

        if !status.success() {
            return Err(anyhow!("`{}` exited with {status}", render_command(&child)));
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

    #[test]
    fn profile_defaults_to_none() {
        let inputs = parse(r#"{"command":"build","build_target":"workspace"}"#);
        assert!(inputs.profile.is_none());
    }

    #[test]
    fn profile_passed_through() {
        let inputs = parse(r#"{"command":"build","build_target":"workspace","profile":"release"}"#);
        assert_eq!(inputs.profile.as_deref(), Some("release"));
    }

    #[test]
    fn target_selectors_default_to_none() {
        let inputs = parse(r#"{"command":"build","build_target":"workspace"}"#);
        assert!(inputs.examples.is_none());
        assert!(inputs.benches.is_none());
        assert!(inputs.tests.is_none());
        assert!(inputs.bins.is_none());
    }

    #[test]
    fn target_all_from_string() {
        let inputs = parse(r#"{"command":"build","build_target":"workspace","examples":"all"}"#);
        assert!(matches!(
            inputs.examples,
            Some(TargetSpecifier::All(AllTag::All))
        ));
    }

    #[test]
    fn target_names_from_array() {
        let inputs = parse(
            r#"{"command":"test","build_target":"workspace","tests":["integration","smoke"]}"#,
        );
        match inputs.tests {
            Some(TargetSpecifier::Names(names)) => {
                assert_eq!(names, vec!["integration", "smoke"]);
            }
            _ => panic!("expected Names variant"),
        }
    }

    #[test]
    fn target_all_tag_is_lowercase() {
        assert!(
            serde_json::from_str::<CargoInputs>(
                r#"{"command":"build","build_target":"workspace","bins":"ALL"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn all_targets_and_lib_default_to_false() {
        let inputs = parse(r#"{"command":"build","build_target":"workspace"}"#);
        assert!(!inputs.all_targets);
        assert!(!inputs.lib);
    }

    #[test]
    fn all_targets_and_lib_parsed() {
        let inputs = parse(
            r#"{"command":"build","build_target":"workspace","all_targets":true,"lib":true}"#,
        );
        assert!(inputs.all_targets);
        assert!(inputs.lib);
    }

    #[test]
    fn render_command_joins_program_and_args() {
        let mut cmd = process::Command::new("cargo");
        cmd.arg("build")
            .arg("--workspace")
            .arg("--example")
            .arg("foo");
        assert_eq!(
            render_command(&cmd),
            "cargo build --workspace --example foo"
        );
    }
}
