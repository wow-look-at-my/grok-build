//! The result text of a command that ran under split-and-tee: the tail view.

use std::io::Read;
use std::path::Path;

use super::command_plan::{CapturePlan, TailView};
use crate::implementations::grok_build::task_output::view::stage_exit_codes;
use crate::types::output::BashOutput;
use crate::types::template_renderer::TemplateRenderer;
use crate::types::tool::ToolKind;
use crate::util::truncate::format_bytes;

/// Line and byte counts of a saved file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileCounts {
    pub(crate) lines: u64,
    pub(crate) bytes: u64,
}

pub(crate) fn count_lines(path: &Path) -> Option<FileCounts> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 64 * 1024];
    let (mut lines, mut bytes, mut last) = (0u64, 0u64, b'\n');
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        lines += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
        bytes += n as u64;
        last = buf[n - 1];
    }
    if last != b'\n' {
        lines += 1;
    }
    Some(FileCounts { lines, bytes })
}

/// How the footer names the tool that reads a kept log.
pub(crate) struct ReadBack {
    pub(crate) tool: String,
    pub(crate) task_ids: String,
    pub(crate) stage: String,
    pub(crate) head: String,
    pub(crate) tail: String,
    pub(crate) grep: String,
}

impl ReadBack {
    pub(crate) fn from_renderer(renderer: Option<&TemplateRenderer>) -> Option<Self> {
        let renderer = renderer?;
        let kind = ToolKind::BackgroundTaskAction;
        let param = |name: &'static str| {
            renderer
                .param_for_kind(kind, name)
                .unwrap_or(name)
                .to_string()
        };
        Some(Self {
            tool: renderer.tool_for_kind(kind)?.to_string(),
            task_ids: param("task_ids"),
            stage: param("stage"),
            head: param("head"),
            tail: param("tail"),
            grep: param("grep"),
        })
    }
}

/// Everything the report needs that comes from disk after the run.
pub(crate) struct CaptureFacts {
    pub(crate) stages: Vec<Option<FileCounts>>,
    pub(crate) stage_codes: Vec<i32>,
    pub(crate) output: Option<FileCounts>,
}

impl CaptureFacts {
    pub(crate) fn gather(plan: &CapturePlan, log_file: &Path) -> Self {
        Self {
            stages: plan.stages.iter().map(|s| count_lines(&s.file)).collect(),
            stage_codes: stage_exit_codes(log_file),
            output: count_lines(log_file),
        }
    }
}

fn short(command: &str) -> String {
    let one_line = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 60 {
        return one_line;
    }
    let cut: String = one_line.chars().take(57).collect();
    format!("{cut}...")
}

fn counts(c: Option<FileCounts>) -> String {
    match c {
        Some(c) if c.lines == 1 => format!("1 line, {}", format_bytes(c.bytes)),
        Some(c) => format!("{} lines, {}", c.lines, format_bytes(c.bytes)),
        None => "nothing saved".to_string(),
    }
}

fn exit(code: Option<&i32>) -> String {
    code.map_or_else(String::new, |c| format!("exit {c}, "))
}

/// Rewrite `bash.output_for_prompt` for a command that ran under `plan`.
pub(crate) fn apply(
    bash: &mut BashOutput,
    plan: &CapturePlan,
    facts: &CaptureFacts,
    read_back: Option<&ReadBack>,
    task_id: &str,
) {
    if let Some(view) = plan.tail_view {
        // The first line is the `exit:` header. Its truncation note describes
        // the full output, which the view does not show.
        let header = {
            let mut plain = bash.clone();
            plain.truncated = false;
            super::format_default_prompt(&plain)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let raw = String::from_utf8_lossy(&bash.output);
        let stripped = strip_ansi_escapes::strip_str(&raw);
        bash.output_for_prompt = format!(
            "{header}\n{}",
            BashOutput::make_output_for_prompt(view.apply(&stripped))
        );
    }
    bash.output_for_prompt
        .push_str(&footer(plan, facts, read_back, task_id, &bash.output_file));
}

fn footer(
    plan: &CapturePlan,
    facts: &CaptureFacts,
    read_back: Option<&ReadBack>,
    task_id: &str,
    output_file: &str,
) -> String {
    let mut out = String::from("\n");
    if let Some(view) = plan.tail_view {
        let total = facts.output.map(|c| c.lines);
        let shown = match (view, total) {
            (TailView::Last(n), Some(t)) if t <= n as u64 => format!("all {t} lines shown"),
            (TailView::Last(n), Some(t)) => format!("the last {n} of {t} lines shown"),
            (TailView::From(n), Some(t)) => format!("line {n} on, of {t} lines, shown"),
            (TailView::Last(n), None) => format!("the last {n} lines shown"),
            (TailView::From(n), None) => format!("line {n} on shown"),
        };
        out.push_str(&format!(
            "\n[`{}` ran as a view over the kept output: {shown}. The exit code is `{}`'s own.]",
            view.describe(),
            short(&plan.final_stage),
        ));
    }
    if !plan.stages.is_empty() {
        out.push_str("\n[every stage of this pipe was kept]");
        for (stage, count) in plan.stages.iter().zip(&facts.stages) {
            out.push_str(&format!(
                "\n  stage {} `{}`: {}{}",
                stage.index,
                short(&stage.command),
                exit(facts.stage_codes.get(stage.index - 1)),
                counts(*count),
            ));
        }
        let last = plan.stages.len() + 1;
        out.push_str(&format!(
            "\n  stage {last} `{}`: {}the output above",
            short(&plan.final_stage),
            exit(facts.stage_codes.get(last - 1)),
        ));
    }
    match read_back {
        Some(rb) => {
            let example = match plan.stages.first() {
                Some(s) => format!(
                    "{}(\"{}\": [\"{task_id}\"], \"{}\": {}, \"{}\": 100)",
                    rb.tool, rb.task_ids, rb.stage, s.index, rb.tail
                ),
                None => format!(
                    "{}(\"{}\": [\"{task_id}\"], \"{}\": 200)",
                    rb.tool, rb.task_ids, rb.tail
                ),
            };
            out.push_str(&format!(
                "\nRead it with {example}. \"{}\": \"<regex>\" and \"{}\": N also filter it. Do \
				 not run the command again to see more of its output.",
                rb.grep, rb.head
            ));
        }
        None => {
            out.push_str(&format!("\nFull output: {output_file}"));
            for s in &plan.stages {
                out.push_str(&format!("\nStage {}: {}", s.index, s.file.display()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implementations::grok_build::bash::command_plan::plan_pipeline_capture;

    fn read_back() -> ReadBack {
        ReadBack {
            tool: "get_task_output".into(),
            task_ids: "task_ids".into(),
            stage: "stage".into(),
            head: "head".into(),
            tail: "tail".into(),
            grep: "grep".into(),
        }
    }

    #[test]
    fn footer_lists_every_stage_and_the_call_that_reads_it() {
        let plan =
            plan_pipeline_capture("cargo test 2>&1 | grep FAIL | sort", Path::new("/s/c1.log"))
                .unwrap();
        let facts = CaptureFacts {
            stages: vec![
                Some(FileCounts {
                    lines: 4213,
                    bytes: 2048,
                }),
                Some(FileCounts { lines: 0, bytes: 0 }),
            ],
            stage_codes: vec![101, 1, 0],
            output: Some(FileCounts { lines: 0, bytes: 0 }),
        };
        let text = footer(&plan, &facts, Some(&read_back()), "c1", "/s/c1.log");
        assert!(
            text.contains("stage 1 `cargo test 2>&1`: exit 101, 4213 lines, 2.0 KB"),
            "{text}"
        );
        assert!(
            text.contains("stage 2 `grep FAIL`: exit 1, 0 lines"),
            "{text}"
        );
        assert!(
            text.contains("stage 3 `sort`: exit 0, the output above"),
            "{text}"
        );
        assert!(
            text.contains(r#"get_task_output("task_ids": ["c1"], "stage": 1, "tail": 100)"#),
            "{text}"
        );
    }

    #[test]
    fn footer_names_the_tail_view_and_the_real_exit_code() {
        let plan = plan_pipeline_capture("make 2>&1 | tail -n 20", Path::new("/s/c2.log")).unwrap();
        let facts = CaptureFacts {
            stages: vec![],
            stage_codes: vec![],
            output: Some(FileCounts {
                lines: 900,
                bytes: 9000,
            }),
        };
        let text = footer(&plan, &facts, Some(&read_back()), "c2", "/s/c2.log");
        assert!(
			text.contains("`tail -n 20` ran as a view over the kept output: the last 20 of 900 lines shown. The exit code is `make 2>&1`'s own."),
			"{text}"
		);
        assert!(
            text.contains(r#"get_task_output("task_ids": ["c2"], "tail": 200)"#),
            "{text}"
        );
    }

    #[test]
    fn footer_falls_back_to_paths_without_a_task_output_tool() {
        let plan = plan_pipeline_capture("a | b", Path::new("/s/c3.log")).unwrap();
        let facts = CaptureFacts {
            stages: vec![None],
            stage_codes: vec![],
            output: None,
        };
        let text = footer(&plan, &facts, None, "c3", "/s/c3.log");
        assert!(text.contains("Stage 1: /s/c3.stage1.log"), "{text}");
        assert!(text.contains("stage 1 `a`: nothing saved"), "{text}");
    }

    #[test]
    fn count_lines_counts_an_unterminated_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, "a\nb").unwrap();
        assert_eq!(count_lines(&p), Some(FileCounts { lines: 2, bytes: 3 }));
        std::fs::write(&p, "").unwrap();
        assert_eq!(count_lines(&p), Some(FileCounts { lines: 0, bytes: 0 }));
    }
}
