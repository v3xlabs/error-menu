use std::collections::{BTreeMap, BTreeSet};

use crate::confidence::Confidence;
use crate::finding::fingerprint::{Components, Fingerprint};
use crate::finding::{Attribution, LineSpan, Location, NewFinding, Severity};
use crate::vcs::RepoPath;

pub const ANALYZER: &str = "workflow-security";

const UNPINNED_ACTION: &str = "unpinned-action";
const PERMISSION_WIDENING: &str = "permission-widening";
const UNSAFE_UNTRUSTED_CHECKOUT: &str = "unsafe-untrusted-checkout";
const SHELL_PIPE: &str = "shell-pipe";

pub fn is_workflow(path: &RepoPath) -> bool {
    path.as_str().strip_prefix(".github/workflows/").is_some_and(|name| {
        name.ends_with(".yml") || name.ends_with(".yaml")
    })
}

pub fn delta(path: &RepoPath, base: Option<&str>, head: &str) -> Vec<NewFinding> {
    let previous = base.map(observations).unwrap_or_default();
    let previous = previous
        .iter()
        .map(Observation::key)
        .collect::<BTreeSet<_>>();
    let mut occurrences = BTreeMap::new();

    observations(head)
        .into_iter()
        .filter(|observation| !previous.contains(&observation.key()))
        .map(|observation| finding(path, observation, &mut occurrences))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Observation {
    rule: &'static str,
    line: u32,
    title: &'static str,
    detail: String,
    severity: Severity,
    identity: String,
}

impl Observation {
    fn key(&self) -> String {
        format!("{}:{}", self.rule, self.identity)
    }
}

fn observations(text: &str) -> Vec<Observation> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut found = unpinned_actions(&lines);
    found.extend(permission_widenings(&lines));
    found.extend(unsafe_untrusted_checkouts(&lines));
    found.extend(shell_pipes(&lines));
    found
}

fn unpinned_actions(lines: &[&str]) -> Vec<Observation> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let reference = action_reference(line)?;
            let (_, revision) = reference.rsplit_once('@')?;
            if is_pinned(revision) || reference.contains("${{") {
                return None;
            }

            Some(Observation {
                rule: UNPINNED_ACTION,
                line: index as u32 + 1,
                title: "workflow action is not pinned to a commit",
                detail: format!("{reference} does not name a full commit SHA."),
                severity: Severity::Medium,
                identity: reference.to_owned(),
            })
        })
        .collect()
}

fn permission_widenings(lines: &[&str]) -> Vec<Observation> {
    let mut found = Vec::new();
    let mut permissions_indent = None;

    for (index, line) in lines.iter().enumerate() {
        let clean = uncomment(line);
        let indentation = indent(clean);
        let trimmed = clean.trim();

        if let Some(active_indent) = permissions_indent {
            if !trimmed.is_empty() && indentation <= active_indent {
                permissions_indent = None;
            }
        }

        if let Some(value) = trimmed.strip_prefix("permissions:") {
            let value = yaml_value(value);
            if is_write_permission(value) {
                found.push(permission_observation(index, "workflow", value));
            }
            if value.is_empty() {
                permissions_indent = Some(indentation);
            }
            continue;
        }

        let Some(active_indent) = permissions_indent else {
            continue;
        };
        if indentation <= active_indent || trimmed.is_empty() {
            continue;
        }
        let Some((scope, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = yaml_value(value);
        if is_write_permission(value) {
            found.push(permission_observation(index, scope.trim(), value));
        }
    }

    found
}

fn permission_observation(index: usize, scope: &str, value: &str) -> Observation {
    Observation {
        rule: PERMISSION_WIDENING,
        line: index as u32 + 1,
        title: "workflow permission grants write access",
        detail: format!("{scope}: {value} grants write access."),
        severity: Severity::High,
        identity: format!("{scope}:{value}"),
    }
}

fn unsafe_untrusted_checkouts(lines: &[&str]) -> Vec<Observation> {
    let triggers = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| untrusted_trigger(line).map(|trigger| (index as u32 + 1, trigger)))
        .collect::<Vec<_>>();
    if triggers.is_empty() {
        return Vec::new();
    }

    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(action) = action_reference(line) else {
            continue;
        };
        let Some((owner, _)) = action.split_once('@') else {
            continue;
        };
        if owner != "actions/checkout" {
            continue;
        }

        let action_indent = indent(uncomment(line));
        let Some((line, reference)) = checkout_head_ref(lines, index, action_indent) else {
            continue;
        };

        for (_, trigger) in &triggers {
            found.push(Observation {
                rule: UNSAFE_UNTRUSTED_CHECKOUT,
                line,
                title: "untrusted workflow trigger checks out pull-request code",
                detail: format!("{trigger} checks out {reference}."),
                severity: Severity::Critical,
                identity: format!("{trigger}:{reference}"),
            });
        }
    }

    found
}

fn checkout_head_ref(lines: &[&str], action_index: usize, action_indent: usize) -> Option<(u32, String)> {
    for (index, line) in lines.iter().enumerate().skip(action_index + 1) {
        let clean = uncomment(line);
        let trimmed = clean.trim();
        let indentation = indent(clean);
        if !trimmed.is_empty() && indentation < action_indent {
            return None;
        }
        if indentation == action_indent && trimmed.starts_with("- ") {
            return None;
        }
        let Some(value) = trimmed.strip_prefix("ref:") else {
            continue;
        };
        let value = yaml_value(value);
        if value.contains("github.event.pull_request.head.") {
            return Some((index as u32 + 1, value.to_owned()));
        }
    }

    None
}

fn shell_pipes(lines: &[&str]) -> Vec<Observation> {
    let mut found = Vec::new();
    let mut run_indent = None;

    for (index, line) in lines.iter().enumerate() {
        let clean = uncomment(line);
        let trimmed = clean.trim();
        let indentation = indent(clean);

        if let Some(active_indent) = run_indent {
            if !trimmed.is_empty() && indentation <= active_indent {
                run_indent = None;
            }
        }

        let Some(value) = trimmed.strip_prefix("run:").or_else(|| trimmed.strip_prefix("- run:")) else {
            if run_indent.is_some_and(|active_indent| indentation > active_indent) && is_shell_pipe(trimmed) {
                found.push(shell_pipe_observation(index, trimmed));
            }
            continue;
        };
        let value = yaml_value(value);
        if matches!(value, "|" | ">" | "|-" | ">-") {
            run_indent = Some(indentation);
        } else if is_shell_pipe(value) {
            found.push(shell_pipe_observation(index, value));
        }
    }

    found
}

fn shell_pipe_observation(index: usize, command: &str) -> Observation {
    Observation {
        rule: SHELL_PIPE,
        line: index as u32 + 1,
        title: "workflow pipes downloaded content into a shell",
        detail: format!("{command} executes downloaded content through a shell."),
        severity: Severity::High,
        identity: command.to_owned(),
    }
}

fn action_reference(line: &str) -> Option<&str> {
    let (_, value) = uncomment(line).split_once("uses:")?;
    let reference = yaml_value(value).split_ascii_whitespace().next()?;

    (reference.contains('/') && reference.contains('@')).then_some(reference)
}

fn untrusted_trigger(line: &str) -> Option<&'static str> {
    let trigger = uncomment(line).trim_start().strip_suffix(':')?;

    match trigger.trim() {
        "pull_request_target" => Some("pull_request_target"),
        "issue_comment" => Some("issue_comment"),
        _ => None,
    }
}

fn is_pinned(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_write_permission(value: &str) -> bool {
    matches!(value, "write" | "write-all")
}

fn is_shell_pipe(value: &str) -> bool {
    let Some(pipe) = value.find('|') else {
        return false;
    };
    let download = &value[..pipe];
    if !contains_command(download, "curl") && !contains_command(download, "wget") {
        return false;
    }

    let shell = value[pipe + 1..].trim_start();
    let shell = shell.strip_prefix("sudo ").unwrap_or(shell);
    ["sh", "bash", "zsh", "dash", "ksh"]
        .iter()
        .any(|candidate| shell == *candidate || shell.starts_with(&format!("{candidate} ")))
}

fn contains_command(value: &str, command: &str) -> bool {
    value.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|word| word == command)
}

fn uncomment(line: &str) -> &str {
    line.split('#').next().unwrap_or_default()
}

fn yaml_value(value: &str) -> &str {
    value.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn finding(
    path: &RepoPath,
    observation: Observation,
    occurrences: &mut BTreeMap<&'static str, u32>,
) -> NewFinding {
    let occurrence = occurrences.entry(observation.rule).or_insert(0);
    let location = Location::File {
        path: path.clone(),
        span: Some(LineSpan {
            start: observation.line,
            end: observation.line,
        }),
    };
    let finding = NewFinding {
        movement: None,
        fingerprint: Fingerprint::compute(&Components {
            analyzer: ANALYZER,
            rule: observation.rule,
            location: &location,
            title: observation.title,
            occurrence: *occurrence,
        }),
        location,
        severity: observation.severity,
        confidence: Confidence::new(1.0).expect("one is in range"),
        attribution: Attribution::Introduced,
        title: observation.title.to_owned(),
        detail: observation.detail,
    };

    *occurrence += 1;

    finding
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> RepoPath {
        RepoPath::new(".github/workflows/release.yml").expect("path is valid")
    }

    #[test]
    fn reports_concrete_unpinned_actions_but_not_sha_pins_or_expressions() {
        let findings = delta(
            &path(),
            None,
            "steps:\n  - uses: actions/checkout@v4\n  - uses: acme/deploy@0123456789abcdef0123456789abcdef01234567\n  - uses: acme/task@${{ inputs.ref }}\n",
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "workflow action is not pinned to a commit");
        assert!(findings[0].detail.contains("actions/checkout@v4"));
    }

    #[test]
    fn reports_new_write_permissions() {
        let findings = delta(
            &path(),
            Some("permissions:\n  contents: read\n"),
            "permissions:\n  contents: write\n",
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "workflow permission grants write access");
        assert_eq!(findings[0].severity, Severity::High);
    }

    #[test]
    fn reports_untrusted_checkout_only_when_the_checkout_ref_is_concrete() {
        let findings = delta(
            &path(),
            None,
            "on:\n  pull_request_target:\njobs:\n  check:\n    steps:\n      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567\n        with:\n          ref: ${{ github.event.pull_request.head.sha }}\n",
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "untrusted workflow trigger checks out pull-request code");
        assert_eq!(findings[0].severity, Severity::Critical);
    }

    #[test]
    fn reports_downloaded_shell_pipelines_in_run_blocks() {
        let findings = delta(
            &path(),
            None,
            "steps:\n  - run: |\n      curl https://example.test/install | bash\n  - run: echo safe\n",
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "workflow pipes downloaded content into a shell");
    }

    #[test]
    fn does_not_report_unchanged_workflow_debt() {
        let workflow = "steps:\n  - uses: actions/checkout@v4\n";

        assert!(delta(&path(), Some(workflow), workflow).is_empty());
    }
}
