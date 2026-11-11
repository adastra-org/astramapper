/** Turns a `Report` into the short markdown table that goes in a PR or a chat. */
import type { Report } from "./runner";

function num(value: number | null): string {
  if (value === null || Number.isNaN(value)) return "n/a";
  if (Number.isInteger(value)) return String(value);
  return Math.abs(value) >= 100 ? value.toFixed(0) : value.toFixed(2);
}

function cell(text: string): string {
  return text.split("|").join("\\|");
}

export function renderMarkdown(report: Report): string {
  const out: string[] = [];
  out.push(`# ${report.scenario}: ${report.ok ? "PASS" : "FAIL"}`);
  out.push("");
  out.push(report.description);
  out.push("");
  out.push(`Started ${report.startedAt}, ${(report.durationMs / 1000).toFixed(1)} s.`);
  out.push("");

  out.push("## Assertions");
  out.push("");
  if (report.assertions.length === 0) {
    out.push("None ran.");
  } else {
    out.push("| | Assertion | Measured | Limit | Note |");
    out.push("|---|---|---|---|---|");
    for (const a of report.assertions) {
      out.push(
        `| ${a.ok ? "pass" : "FAIL"} | ${cell(a.label)} | ${num(a.actual)} | ${a.op} ${num(a.expected)} | ${cell(a.detail ?? "")} |`,
      );
    }
  }
  out.push("");

  const failedSteps = report.steps.filter((s) => !s.ok);
  if (failedSteps.length > 0) {
    out.push("## Failed steps");
    out.push("");
    for (const s of failedSteps) out.push(`- ${s.index + 1}. ${s.label}: ${s.error ?? "failed"}`);
    out.push("");
  }

  if (report.series.length > 0) {
    out.push("## Samples");
    out.push("");
    out.push("| Sample | Series | n | first | last | min | max | mean | per min |");
    out.push("|---|---|---|---|---|---|---|---|---|");
    for (const s of report.series) {
      out.push(
        `| ${cell(s.sample)} | ${cell(s.key)} | ${s.points} | ${num(s.first)} | ${num(s.last)} | ${num(s.min)} | ${num(s.max)} | ${num(s.mean)} | ${num(s.slopePerMin)} |`,
      );
    }
    out.push("");
  }

  if (report.screenshots.length > 0) {
    out.push("## Screenshots");
    out.push("");
    for (const file of report.screenshots) out.push(`- ${file}`);
    out.push("");
  }

  out.push(
    `Console during the run: ${report.console.errors} errors, ${report.console.warnings} warnings.`,
  );
  for (const line of report.console.sample) out.push(`- ${cell(line)}`);
  out.push("");
  return out.join("\n");
}
