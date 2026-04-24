// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dynamic workflow definition discovery from YAML files.
//!
//! Scans a designated directory for `.yml` files, parses them into
//! [`DiscoveredWorkflow`] structs, and validates schema, dependency
//! references, and circular dependencies.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::{AgentStepConfig, SkillRef, StepAvailability};

// ── YAML schema types ───────────────────────────────────────────────────────

/// Raw YAML schema for a workflow definition file.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowYaml {
    /// Human-readable display name shown on the workflow button.
    pub name: String,
    /// Ordered list of execution steps.
    pub steps: Vec<WorkflowStepYaml>,
    /// Optional list of prerequisite workflow filenames (without `.yml` extension).
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// A single step in a YAML workflow definition.
///
/// Supports two forms:
/// - **Short form**: `{ run: "command" }` → becomes a command step
/// - **Full form**: mirrors [`AgentStepConfig`] with name, prompt, commands, etc.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowStepYaml {
    /// Short form: a command string to execute (becomes a command step with name = run value).
    #[serde(default)]
    pub run: Option<String>,

    /// Full form fields — same as `AgentStepConfig`.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
    #[serde(default)]
    pub repeat: Option<u8>,
    #[serde(default)]
    pub skills: Option<Vec<SkillRefYaml>>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub resume_previous: Option<bool>,
}

/// Skill reference in YAML format.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillRefYaml {
    pub name: String,
    #[serde(default)]
    pub args: Vec<String>,
}

// ── Discovery result types ──────────────────────────────────────────────────

/// A single discovered workflow definition, ready for use by the engine and API.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredWorkflow {
    /// Filename without the `.yml` extension (used for dependency resolution).
    pub filename: String,
    /// Human-readable display name from the `name` field.
    pub name: String,
    /// Converted steps ready for execution.
    pub steps: Vec<AgentStepConfig>,
    /// Filenames (without `.yml`) of workflows that must complete before this one.
    pub depends_on: Vec<String>,
    /// Whether this workflow definition is valid and can be executed.
    pub valid: bool,
    /// Validation error message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of scanning a directory for workflow definitions.
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    /// All discovered workflows (including invalid ones with `valid = false`).
    pub workflows: Vec<DiscoveredWorkflow>,
}

// ── Execution state tracking ────────────────────────────────────────────────

/// Run state for a workflow definition within a specific ticket workflow.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkflowDefRunState {
    /// Not yet started.
    #[default]
    Idle,
    /// Currently executing steps.
    Running,
    /// All steps completed successfully.
    Completed,
    /// A step failed; workflow is paused at the error.
    Error { message: String },
}

impl WorkflowDefRunState {
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Error { .. } => "error",
        }
    }
}

// ── Core discovery logic ────────────────────────────────────────────────────

/// Scan `dir` for `.yml` files and parse each as a workflow definition.
///
/// Invalid files are included in the result with `valid = false` and an `error` message.
/// Circular and missing dependencies are detected and flagged.
pub fn discover_workflows(dir: &Path) -> DiscoveryResult {
    let mut workflows = Vec::new();

    if !dir.is_dir() {
        info!(
            path = %dir.display(),
            "Workflows directory does not exist; no workflow definitions discovered"
        );
        return DiscoveryResult { workflows };
    }

    // Collect all .yml files
    let mut yml_files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|ext| ext == "yml" || ext == "yaml")
            })
            .collect(),
        Err(e) => {
            warn!(
                path = %dir.display(),
                error = %e,
                "Failed to read workflows directory"
            );
            return DiscoveryResult { workflows };
        }
    };

    // Sort for deterministic ordering
    yml_files.sort();

    for path in &yml_files {
        let filename = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if filename.is_empty() {
            continue;
        }

        // Skip .example.yml files — they are templates, not active workflows
        let full_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if full_name.contains(".example.") {
            continue;
        }

        match parse_workflow_file(path) {
            Ok(wf) => {
                let steps = convert_steps(&wf.steps, &filename);
                match steps {
                    Ok(steps) => {
                        workflows.push(DiscoveredWorkflow {
                            filename,
                            name: wf.name,
                            steps,
                            depends_on: wf.depends_on,
                            valid: true,
                            error: None,
                        });
                    }
                    Err(e) => {
                        warn!(
                            file = %path.display(),
                            error = %e,
                            "Invalid step schema in workflow definition"
                        );
                        workflows.push(DiscoveredWorkflow {
                            filename,
                            name: wf.name,
                            steps: Vec::new(),
                            depends_on: wf.depends_on,
                            valid: false,
                            error: Some(e),
                        });
                    }
                }
            }
            Err(e) => {
                warn!(
                    file = %path.display(),
                    error = %e,
                    "Failed to parse workflow definition"
                );
                workflows.push(DiscoveredWorkflow {
                    filename: filename.clone(),
                    name: format!("{filename} (invalid)"),
                    steps: Vec::new(),
                    depends_on: Vec::new(),
                    valid: false,
                    error: Some(e),
                });
            }
        }
    }

    // Validate dependencies (circular + missing references)
    validate_dependencies(&mut workflows);

    info!(
        count = workflows.len(),
        valid = workflows.iter().filter(|w| w.valid).count(),
        "Discovered workflow definitions"
    );

    DiscoveryResult { workflows }
}

/// Parse a single `.yml` file into a [`WorkflowYaml`].
fn parse_workflow_file(path: &Path) -> std::result::Result<WorkflowYaml, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))?;

    let wf: WorkflowYaml =
        serde_yaml::from_str(&content).map_err(|e| format!("Invalid YAML schema: {e}"))?;

    // Validate required fields
    if wf.name.trim().is_empty() {
        return Err("'name' field is required and must not be empty".to_string());
    }
    if wf.steps.is_empty() {
        return Err("'steps' field is required and must contain at least one step".to_string());
    }

    Ok(wf)
}

/// Convert YAML steps to [`AgentStepConfig`] structs.
fn convert_steps(
    yaml_steps: &[WorkflowStepYaml],
    workflow_filename: &str,
) -> std::result::Result<Vec<AgentStepConfig>, String> {
    let mut steps = Vec::with_capacity(yaml_steps.len());

    for (i, ys) in yaml_steps.iter().enumerate() {
        let step = convert_single_step(ys, i, workflow_filename)?;
        steps.push(step);
    }

    Ok(steps)
}

/// Convert a single YAML step to an [`AgentStepConfig`].
fn convert_single_step(
    ys: &WorkflowStepYaml,
    index: usize,
    workflow_filename: &str,
) -> std::result::Result<AgentStepConfig, String> {
    // Short form: { run: "command" }
    if let Some(ref run_cmd) = ys.run {
        if ys.prompt.is_some() || ys.commands.is_some() {
            return Err(format!(
                "Step {} in '{}': 'run' shorthand cannot be combined with 'prompt' or 'commands'",
                index + 1,
                workflow_filename
            ));
        }
        let name = ys.name.clone().unwrap_or_else(|| run_cmd.clone());
        return Ok(AgentStepConfig {
            name,
            prompt: String::new(),
            repeat: ys.repeat.unwrap_or(1),
            skills: Vec::new(),
            resume_previous: ys.resume_previous.unwrap_or(false),
            when: parse_step_availability(ys.when.as_deref()),
            commands: vec![run_cmd.clone()],
        });
    }

    // Full form
    let name = ys
        .name
        .clone()
        .unwrap_or_else(|| format!("Step {}", index + 1));

    let prompt = ys.prompt.clone().unwrap_or_default();
    let commands = ys.commands.clone().unwrap_or_default();
    let skills: Vec<SkillRef> = ys
        .skills
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| SkillRef {
            name: s.name.clone(),
            args: s.args.clone(),
        })
        .collect();

    // Validate mutual exclusivity
    let has_commands = !commands.is_empty();
    let has_prompt = !prompt.trim().is_empty();
    let has_skills = !skills.is_empty();

    if has_commands && (has_prompt || has_skills) {
        return Err(format!(
            "Step '{}' in '{}': cannot specify both 'commands' and 'prompt'/'skills' (mutually exclusive)",
            name, workflow_filename
        ));
    }
    if !has_commands && !has_prompt && !has_skills {
        return Err(format!(
            "Step '{}' in '{}': must have 'run', 'prompt', 'skills', or 'commands'",
            name, workflow_filename
        ));
    }

    Ok(AgentStepConfig {
        name,
        prompt,
        repeat: ys.repeat.unwrap_or(1),
        skills,
        resume_previous: ys.resume_previous.unwrap_or(false),
        when: parse_step_availability(ys.when.as_deref()),
        commands,
    })
}

/// Parse a `when` string into a [`StepAvailability`].
fn parse_step_availability(when: Option<&str>) -> StepAvailability {
    match when {
        Some("ticketing") => StepAvailability::Ticketing,
        Some("no_ticketing") => StepAvailability::NoTicketing,
        _ => StepAvailability::Always,
    }
}

/// Validate dependency references and detect circular dependencies.
///
/// Workflows with missing or circular dependencies are marked as invalid.
fn validate_dependencies(workflows: &mut [DiscoveredWorkflow]) {
    // Build a set of valid filenames (owned Strings to avoid borrow conflict)
    let valid_filenames: HashSet<String> = workflows
        .iter()
        .filter(|w| w.valid)
        .map(|w| w.filename.clone())
        .collect();

    // Check for missing dependency references — collect indices to update
    let mut missing_dep_errors: Vec<(usize, String)> = Vec::new();
    for (i, wf) in workflows.iter().enumerate() {
        if !wf.valid {
            continue;
        }
        for dep in &wf.depends_on {
            if !valid_filenames.contains(dep) {
                warn!(
                    workflow = %wf.filename,
                    missing_dep = %dep,
                    "Workflow references a dependency that does not exist"
                );
                missing_dep_errors.push((
                    i,
                    format!(
                        "Missing dependency: '{}' does not match any discovered workflow filename",
                        dep
                    ),
                ));
                break;
            }
        }
    }
    for (i, err) in missing_dep_errors {
        workflows[i].valid = false;
        workflows[i].error = Some(err);
    }

    // Detect circular dependencies using DFS (owned data to avoid borrows)
    let dep_map: HashMap<String, Vec<String>> = workflows
        .iter()
        .filter(|w| w.valid)
        .map(|w| (w.filename.clone(), w.depends_on.clone()))
        .collect();

    let circular = detect_cycles(&dep_map);
    if !circular.is_empty() {
        warn!(
            affected = ?circular,
            "Circular dependency detected among workflow definitions"
        );
        for wf in workflows.iter_mut() {
            if circular.contains(&wf.filename) {
                wf.valid = false;
                wf.error = Some(format!(
                    "Circular dependency detected: workflow '{}' is part of a dependency cycle",
                    wf.filename
                ));
            }
        }
    }
}

/// Detect cycles in the dependency graph using DFS.
/// Returns the set of filenames that are part of at least one cycle.
fn detect_cycles(dep_map: &HashMap<String, Vec<String>>) -> HashSet<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let mut colors: HashMap<&str, Color> =
        dep_map.keys().map(|k| (k.as_str(), Color::White)).collect();
    let mut in_cycle: HashSet<String> = HashSet::new();

    fn dfs(
        node: &str,
        dep_map: &HashMap<String, Vec<String>>,
        colors: &mut HashMap<&str, Color>,
        path: &mut Vec<String>,
        in_cycle: &mut HashSet<String>,
    ) {
        // Safety: we know the node key lives in dep_map for the duration
        // Use a raw pointer trick to satisfy borrow checker
        let node_owned = node.to_string();
        if let Some(color) = colors.get_mut(node) {
            *color = Color::Gray;
        }
        path.push(node_owned);

        if let Some(deps) = dep_map.get(node) {
            for dep in deps {
                let dep_str = dep.as_str();
                let color = colors.get(dep_str).copied();
                match color {
                    Some(Color::Gray) => {
                        // Found a cycle — mark all nodes in the cycle path
                        let cycle_start = path.iter().position(|n| n == dep_str).unwrap_or(0);
                        for n in &path[cycle_start..] {
                            in_cycle.insert(n.clone());
                        }
                    }
                    Some(Color::White) => {
                        dfs(dep_str, dep_map, colors, path, in_cycle);
                    }
                    _ => {}
                }
            }
        }

        path.pop();
        if let Some(color) = colors.get_mut(node) {
            *color = Color::Black;
        }
    }

    let keys: Vec<String> = dep_map.keys().cloned().collect();
    for node in &keys {
        if colors.get(node.as_str()) == Some(&Color::White) {
            let mut path = Vec::new();
            dfs(node, dep_map, &mut colors, &mut path, &mut in_cycle);
        }
    }

    in_cycle
}

/// Check whether all dependencies of a workflow are satisfied (completed).
pub fn are_dependencies_met(
    workflow_filename: &str,
    workflows: &[DiscoveredWorkflow],
    run_states: &HashMap<String, WorkflowDefRunState>,
) -> bool {
    let wf = workflows.iter().find(|w| w.filename == workflow_filename);
    let Some(wf) = wf else { return false };

    if wf.depends_on.is_empty() {
        return true;
    }

    wf.depends_on
        .iter()
        .all(|dep| run_states.get(dep).is_some_and(|s| s.is_completed()))
}

/// Compute a topological ordering of valid workflows for display purposes.
/// Returns filenames in dependency order (dependencies before dependents).
pub fn topological_order(workflows: &[DiscoveredWorkflow]) -> Vec<String> {
    let valid: Vec<&DiscoveredWorkflow> = workflows.iter().filter(|w| w.valid).collect();
    let name_set: HashSet<&str> = valid.iter().map(|w| w.filename.as_str()).collect();

    // Kahn's algorithm
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for wf in &valid {
        in_degree.entry(wf.filename.as_str()).or_insert(0);
        for dep in &wf.depends_on {
            if name_set.contains(dep.as_str()) {
                adj.entry(dep.as_str())
                    .or_default()
                    .push(wf.filename.as_str());
                *in_degree.entry(wf.filename.as_str()).or_insert(0) += 1;
            }
        }
    }

    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| *name)
        .collect();
    queue.sort();

    let mut order = Vec::new();
    while !queue.is_empty() {
        queue.sort(); // Deterministic ordering
        let node = queue.remove(0);
        order.push(node.to_string());
        if let Some(children) = adj.get(node) {
            for &child in children {
                if let Some(deg) = in_degree.get_mut(child) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(child);
                    }
                }
            }
        }
    }

    order
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn discover_empty_directory() {
        let dir = create_temp_dir();
        let result = discover_workflows(dir.path());
        assert!(result.workflows.is_empty());
    }

    #[test]
    fn discover_nonexistent_directory() {
        let result = discover_workflows(Path::new("/nonexistent/path"));
        assert!(result.workflows.is_empty());
    }

    #[test]
    fn discover_valid_workflow() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("build.yml"),
            r#"
name: "Build Project"
steps:
  - run: "npm run build"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert_eq!(result.workflows.len(), 1);

        let wf = &result.workflows[0];
        assert_eq!(wf.filename, "build");
        assert_eq!(wf.name, "Build Project");
        assert!(wf.valid);
        assert!(wf.error.is_none());
        assert_eq!(wf.steps.len(), 1);
        assert!(wf.steps[0].is_command_step());
        assert_eq!(wf.steps[0].commands, vec!["npm run build"]);
        assert!(wf.depends_on.is_empty());
    }

    #[test]
    fn discover_workflow_with_dependencies() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("build.yml"),
            r#"
name: "Build"
steps:
  - run: "npm run build"
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("test.yml"),
            r#"
name: "Test"
steps:
  - run: "npm test"
depends_on:
  - "build"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert_eq!(result.workflows.len(), 2);

        let test_wf = result
            .workflows
            .iter()
            .find(|w| w.filename == "test")
            .unwrap();
        assert!(test_wf.valid);
        assert_eq!(test_wf.depends_on, vec!["build"]);
    }

    #[test]
    fn detect_missing_dependency() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("deploy.yml"),
            r#"
name: "Deploy"
steps:
  - run: "deploy.sh"
depends_on:
  - "nonexistent"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert_eq!(result.workflows.len(), 1);
        assert!(!result.workflows[0].valid);
        assert!(
            result.workflows[0]
                .error
                .as_ref()
                .unwrap()
                .contains("Missing dependency")
        );
    }

    #[test]
    fn detect_circular_dependency() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("a.yml"),
            r#"
name: "A"
steps:
  - run: "echo a"
depends_on:
  - "b"
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("b.yml"),
            r#"
name: "B"
steps:
  - run: "echo b"
depends_on:
  - "a"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert_eq!(result.workflows.len(), 2);
        assert!(result.workflows.iter().all(|w| !w.valid));
        assert!(
            result.workflows.iter().all(|w| w
                .error
                .as_ref()
                .unwrap()
                .contains("Circular dependency"))
        );
    }

    #[test]
    fn skip_invalid_yaml() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("bad.yml"),
            "this is not: [valid yaml: at all",
        )
        .unwrap();
        fs::write(
            dir.path().join("good.yml"),
            r#"
name: "Good"
steps:
  - run: "echo good"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert_eq!(result.workflows.len(), 2);

        let bad = result
            .workflows
            .iter()
            .find(|w| w.filename == "bad")
            .unwrap();
        assert!(!bad.valid);

        let good = result
            .workflows
            .iter()
            .find(|w| w.filename == "good")
            .unwrap();
        assert!(good.valid);
    }

    #[test]
    fn skip_example_files() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("build.example.yml"),
            r#"
name: "Build Example"
steps:
  - run: "echo example"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert!(result.workflows.is_empty());
    }

    #[test]
    fn full_form_agent_step() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("implement.yml"),
            r#"
name: "Implement"
steps:
  - name: "Code it"
    prompt: "Implement the feature"
    repeat: 2
    skills:
      - name: "address-ticket"
        args: ["--headless"]
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert_eq!(result.workflows.len(), 1);
        let wf = &result.workflows[0];
        assert!(wf.valid);
        assert_eq!(wf.steps.len(), 1);

        let step = &wf.steps[0];
        assert_eq!(step.name, "Code it");
        assert_eq!(step.prompt, "Implement the feature");
        assert_eq!(step.repeat, 2);
        assert!(!step.is_command_step());
        assert_eq!(step.skills.len(), 1);
        assert_eq!(step.skills[0].name, "address-ticket");
        assert_eq!(step.skills[0].args, vec!["--headless"]);
    }

    #[test]
    fn dependencies_met_check() {
        let workflows = vec![
            DiscoveredWorkflow {
                filename: "build".to_string(),
                name: "Build".to_string(),
                steps: Vec::new(),
                depends_on: Vec::new(),
                valid: true,
                error: None,
            },
            DiscoveredWorkflow {
                filename: "test".to_string(),
                name: "Test".to_string(),
                steps: Vec::new(),
                depends_on: vec!["build".to_string()],
                valid: true,
                error: None,
            },
        ];

        let mut run_states = HashMap::new();

        // Build not completed → test deps not met
        assert!(are_dependencies_met("build", &workflows, &run_states));
        assert!(!are_dependencies_met("test", &workflows, &run_states));

        // Build completed → test deps met
        run_states.insert("build".to_string(), WorkflowDefRunState::Completed);
        assert!(are_dependencies_met("test", &workflows, &run_states));

        // Build in error → test deps not met
        run_states.insert(
            "build".to_string(),
            WorkflowDefRunState::Error {
                message: "fail".to_string(),
            },
        );
        assert!(!are_dependencies_met("test", &workflows, &run_states));
    }

    #[test]
    fn reject_step_with_run_and_prompt() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("bad.yml"),
            r#"
name: "Bad"
steps:
  - run: "echo hi"
    prompt: "also prompt"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert_eq!(result.workflows.len(), 1);
        assert!(!result.workflows[0].valid);
    }

    #[test]
    fn reject_empty_name() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("noname.yml"),
            r#"
name: ""
steps:
  - run: "echo"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert!(!result.workflows[0].valid);
    }

    #[test]
    fn reject_empty_steps() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("nosteps.yml"),
            r#"
name: "No Steps"
steps: []
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        assert!(!result.workflows[0].valid);
    }

    #[test]
    fn command_step_full_form() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("lint.yml"),
            r#"
name: "Lint"
steps:
  - name: "Run linter"
    commands:
      - "npm run lint"
      - "npm run format"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        let wf = &result.workflows[0];
        assert!(wf.valid);
        assert_eq!(wf.steps[0].name, "Run linter");
        assert!(wf.steps[0].is_command_step());
        assert_eq!(wf.steps[0].commands, vec!["npm run lint", "npm run format"]);
    }

    #[test]
    fn when_field_parsing() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("conditional.yml"),
            r#"
name: "Conditional"
steps:
  - name: "Ticketing only"
    prompt: "do stuff"
    when: "ticketing"
  - name: "No ticketing"
    prompt: "do other stuff"
    when: "no_ticketing"
  - name: "Always"
    prompt: "always do"
"#,
        )
        .unwrap();

        let result = discover_workflows(dir.path());
        let wf = &result.workflows[0];
        assert!(wf.valid);
        assert_eq!(wf.steps[0].when, StepAvailability::Ticketing);
        assert_eq!(wf.steps[1].when, StepAvailability::NoTicketing);
        assert_eq!(wf.steps[2].when, StepAvailability::Always);
    }
}
