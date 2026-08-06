#!/usr/bin/env python3
"""
Paired baseline/candidate PDF regression harness.

This is a black-box runner: it feeds the same corpus document to two explicit
extractor commands, records command failures as per-document outcomes, compares
character/word metrics against a reference, and fails the run on missing
predictions or configured per-document regression thresholds.
"""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import hashlib
import io
import json
import os
import pathlib
import shlex
import shutil
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from dataclasses import dataclass
from typing import Any


DEFAULT_TIMEOUT_MS = 60_000
STDERR_EXCERPT_LIMIT = 4_096


@dataclass(frozen=True)
class TextMetrics:
    chars: int
    words: int


@dataclass(frozen=True)
class MetricDeltas:
    baseline_char_delta: int
    baseline_word_delta: int
    candidate_char_delta: int
    candidate_word_delta: int
    char_regression: int
    word_regression: int


@dataclass(frozen=True)
class Thresholds:
    max_char_regression: int = 0
    max_word_regression: int = 0
    max_candidate_char_delta: int | None = None
    max_candidate_word_delta: int | None = None


@dataclass(frozen=True)
class CorpusDocument:
    id: str
    relative_path: str
    path: pathlib.Path
    filename: str
    adversarial: bool = False
    expected_sha256: str | None = None
    actual_sha256: str | None = None


@dataclass(frozen=True)
class HarnessConfig:
    corpus_dir: pathlib.Path
    baseline_rev: str
    baseline_cmd: str
    candidate_rev: str
    candidate_cmd: str
    output: pathlib.Path
    corpus_sha256: pathlib.Path | None = None
    reference_dir: pathlib.Path | None = None
    thresholds_source: pathlib.Path | None = None
    thresholds_default: Thresholds = dataclasses.field(default_factory=Thresholds)
    thresholds_documents: dict[str, dict[str, Any]] = dataclasses.field(default_factory=dict)
    markdown_output: pathlib.Path | None = None
    adversarial_dirs: tuple[pathlib.Path, ...] = ()
    adversarial_fixtures: tuple[pathlib.Path, ...] = ()
    timeout_ms: int = DEFAULT_TIMEOUT_MS
    require_document_bound_commands: bool = False
    require_adversarial_fixtures: bool = False


def sha256_file(path: pathlib.Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def metrics_for_text(text: str) -> TextMetrics:
    return TextMetrics(chars=len(text), words=len(text.split()))


def normalized_output_sha256(text: str) -> str:
    normalized = " ".join(text.split())
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()


def truncate_excerpt(text: str, limit: int = STDERR_EXCERPT_LIMIT) -> str:
    if len(text) <= limit:
        return text
    return text[:limit] + "\n...[truncated]"


def read_text_lossy(path: pathlib.Path) -> str:
    return path.read_bytes().decode("utf-8", errors="replace")


def parse_sha256_manifest(corpus_dir: pathlib.Path, manifest: pathlib.Path) -> list[CorpusDocument]:
    documents: list[CorpusDocument] = []
    for line_no, raw_line in enumerate(manifest.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(maxsplit=1)
        if len(parts) != 2:
            raise ValueError(f"{manifest}:{line_no}: expected '<sha256> <filename>'")
        expected_sha256 = parts[0].lower()
        relative_path = parts[1].strip().removeprefix("*").strip()
        path = corpus_dir / relative_path
        actual_sha256 = sha256_file(path)
        if actual_sha256 != expected_sha256:
            raise ValueError(
                f"corpus hash mismatch for {path}: expected {expected_sha256}, got {actual_sha256}"
            )
        documents.append(
            CorpusDocument(
                id=relative_path,
                relative_path=relative_path,
                path=path,
                filename=path.name,
                expected_sha256=expected_sha256,
                actual_sha256=actual_sha256,
            )
        )
    return documents


def document_from_path(
    path: pathlib.Path, *, adversarial: bool, relative_base: pathlib.Path | None
) -> CorpusDocument:
    relative_path = str(path.relative_to(relative_base)) if relative_base else str(path)
    return CorpusDocument(
        id=relative_path,
        relative_path=relative_path,
        path=path,
        filename=path.name,
        adversarial=adversarial,
        actual_sha256=sha256_file(path) if path.exists() else None,
    )


def load_pdf_dir(
    directory: pathlib.Path, *, adversarial: bool, relative_base: pathlib.Path | None
) -> list[CorpusDocument]:
    return [
        document_from_path(path, adversarial=adversarial, relative_base=relative_base)
        for path in sorted(directory.iterdir(), key=lambda value: value.name)
        if path.is_file() and path.suffix.lower() == ".pdf"
    ]


def load_corpus(config: HarnessConfig) -> list[CorpusDocument]:
    if config.corpus_sha256:
        documents = parse_sha256_manifest(config.corpus_dir, config.corpus_sha256)
    else:
        documents = load_pdf_dir(config.corpus_dir, adversarial=False, relative_base=config.corpus_dir)

    for directory in config.adversarial_dirs:
        documents.extend(load_pdf_dir(directory, adversarial=True, relative_base=directory))
    for fixture in config.adversarial_fixtures:
        documents.append(document_from_path(fixture, adversarial=True, relative_base=fixture.parent))

    if not documents:
        raise ValueError(f"no PDF documents found in {config.corpus_dir}")
    return documents


def corpus_fingerprint(documents: list[CorpusDocument]) -> str:
    hasher = hashlib.sha256()
    for document in documents:
        hasher.update(document.relative_path.encode("utf-8"))
        hasher.update(b"\0")
        hasher.update((document.actual_sha256 or "").encode("ascii"))
        hasher.update(b"\0")
        hasher.update(b"A" if document.adversarial else b"C")
        hasher.update(b"\0")
    return "sha256:" + hasher.hexdigest()


def threshold_from_mapping(base: Thresholds, raw: dict[str, Any] | None) -> Thresholds:
    if not raw:
        return base
    return Thresholds(
        max_char_regression=int(
            raw.get("max_char_regression", raw.get("max_char_delta_regression", base.max_char_regression))
        ),
        max_word_regression=int(
            raw.get("max_word_regression", raw.get("max_word_delta_regression", base.max_word_regression))
        ),
        max_candidate_char_delta=(
            None
            if raw.get("max_candidate_char_delta", base.max_candidate_char_delta) is None
            else int(raw.get("max_candidate_char_delta", base.max_candidate_char_delta))
        ),
        max_candidate_word_delta=(
            None
            if raw.get("max_candidate_word_delta", base.max_candidate_word_delta) is None
            else int(raw.get("max_candidate_word_delta", base.max_candidate_word_delta))
        ),
    )


def load_thresholds(
    path: pathlib.Path | None, cli_default: Thresholds
) -> tuple[Thresholds, dict[str, dict[str, Any]], str]:
    if path is None:
        return cli_default, {}, "cli-defaults"
    raw = json.loads(path.read_text(encoding="utf-8"))
    default = threshold_from_mapping(cli_default, raw.get("default"))
    default = threshold_from_mapping(default, raw.get("defaults"))
    documents = raw.get("documents", {})
    if not isinstance(documents, dict):
        raise ValueError(f"{path}: documents must be an object")
    return default, documents, str(path)


def thresholds_for_document(config: HarnessConfig, document: CorpusDocument) -> Thresholds:
    for key in (document.id, document.relative_path, document.filename):
        if key in config.thresholds_documents:
            return threshold_from_mapping(config.thresholds_default, config.thresholds_documents[key])
    return config.thresholds_default


def expand_command_template(
    template: str, document: CorpusDocument, side: str, output_path: pathlib.Path
) -> str:
    replacements = {
        "{pdf}": shlex.quote(str(document.path)),
        "{pdf_basename}": shlex.quote(document.filename),
        "{doc_id}": shlex.quote(document.id),
        "{side}": shlex.quote(side),
        "{output}": shlex.quote(str(output_path)),
    }
    expanded = template
    for token, value in replacements.items():
        expanded = expanded.replace(token, value)
    return expanded


def prediction_report(
    *,
    status: str,
    exit_code: int | None,
    timed_out: bool,
    wall_ms: float,
    metrics: TextMetrics | None = None,
    normalized_output_sha256_value: str | None = None,
    stderr: str = "",
    error: str | None = None,
) -> dict[str, Any]:
    return {
        "status": status,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "wall_ms": wall_ms,
        "metrics": dataclasses.asdict(metrics) if metrics else None,
        "normalized_output_sha256": normalized_output_sha256_value,
        "stderr_excerpt": truncate_excerpt(stderr.strip()) if stderr.strip() else None,
        "error": error,
    }


def run_extractor(
    *, side: str, template: str, document: CorpusDocument, timeout_ms: int
) -> dict[str, Any]:
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="paired-regression-") as temp_dir:
        output_path = pathlib.Path(temp_dir) / "prediction.txt"
        command = expand_command_template(template, document, side, output_path)
        env = os.environ.copy()
        env.update(
            {
                "PDF_PATH": str(document.path),
                "PDF_BASENAME": document.filename,
                "DOC_ID": document.id,
                "RZN_PAIRED_SIDE": side,
                "RZN_PAIRED_OUTPUT": str(output_path),
            }
        )
        try:
            completed = subprocess.run(
                command,
                shell=True,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=timeout_ms / 1000,
                check=False,
            )
        except subprocess.TimeoutExpired as error:
            wall_ms = (time.monotonic() - started) * 1000
            stderr = (error.stderr or b"").decode("utf-8", errors="replace")
            return prediction_report(
                status="timeout",
                exit_code=None,
                timed_out=True,
                wall_ms=wall_ms,
                stderr=stderr,
                error=f"command timed out after {timeout_ms} ms",
            )

        wall_ms = (time.monotonic() - started) * 1000
        stdout = completed.stdout.decode("utf-8", errors="replace")
        stderr = completed.stderr.decode("utf-8", errors="replace")
        if completed.returncode != 0:
            return prediction_report(
                status="error",
                exit_code=completed.returncode,
                timed_out=False,
                wall_ms=wall_ms,
                stderr=stderr,
                error=f"command exited unsuccessfully with code {completed.returncode}",
            )

        if output_path.exists():
            prediction = read_text_lossy(output_path)
        else:
            prediction = stdout
        if not prediction.strip():
            return prediction_report(
                status="missing_prediction",
                exit_code=completed.returncode,
                timed_out=False,
                wall_ms=wall_ms,
                stderr=stderr,
                error="command produced no prediction",
            )

        metrics = metrics_for_text(prediction)
        return prediction_report(
            status="ok",
            exit_code=completed.returncode,
            timed_out=False,
            wall_ms=wall_ms,
            metrics=metrics,
            normalized_output_sha256_value=normalized_output_sha256(prediction),
            stderr=stderr,
        )


def find_reference_file(reference_dir: pathlib.Path, document: CorpusDocument) -> pathlib.Path | None:
    relative = pathlib.Path(document.relative_path)
    candidates = [
        reference_dir / relative.with_suffix(".txt"),
        reference_dir / f"{document.relative_path}.txt",
        (reference_dir / document.filename).with_suffix(".txt"),
        reference_dir / f"{document.filename}.txt",
    ]
    return next((path for path in candidates if path.exists()), None)


def load_reference(
    config: HarnessConfig, document: CorpusDocument, baseline: dict[str, Any]
) -> dict[str, Any]:
    if config.reference_dir:
        reference_path = find_reference_file(config.reference_dir, document)
        if reference_path is None:
            return {"source": str(config.reference_dir), "metrics": None, "error": "reference_missing"}
        try:
            metrics = metrics_for_text(read_text_lossy(reference_path))
        except OSError as error:
            return {
                "source": str(reference_path),
                "metrics": None,
                "error": f"reference_read_error: {error}",
            }
        return {"source": str(reference_path), "metrics": dataclasses.asdict(metrics), "error": None}

    if baseline.get("metrics"):
        return {
            "source": f"baseline-output:{config.baseline_rev}",
            "metrics": baseline["metrics"],
            "error": None,
        }
    return {
        "source": f"baseline-output:{config.baseline_rev}",
        "metrics": None,
        "error": "baseline_prediction_unavailable",
    }


FATAL_MARKERS = (
    "sigabrt",
    "sigsegv",
    "sigbus",
    "stack overflow",
    "core dumped",
    "illegal instruction",
    "panic",
    "thread 'main' panicked",
)


def failure_for_prediction(
    side: str, prediction: dict[str, Any], *, adversarial: bool
) -> str | None:
    status = prediction["status"]
    if status == "ok":
        return None
    if status == "missing_prediction":
        return f"{side}_missing_prediction"
    if status == "timeout":
        return f"{side}_timeout"
    if prediction_is_crash(prediction):
        return f"{side}_crash"
    if adversarial:
        # A normal parser error is an expected adversarial outcome. Signals,
        # shell signal exit codes, and fatal markers were handled above.
        return None
    return f"{side}_error"


def metric_deltas(
    reference: dict[str, int], baseline: dict[str, int], candidate: dict[str, int]
) -> MetricDeltas:
    baseline_char_delta = baseline["chars"] - reference["chars"]
    baseline_word_delta = baseline["words"] - reference["words"]
    candidate_char_delta = candidate["chars"] - reference["chars"]
    candidate_word_delta = candidate["words"] - reference["words"]
    return MetricDeltas(
        baseline_char_delta=baseline_char_delta,
        baseline_word_delta=baseline_word_delta,
        candidate_char_delta=candidate_char_delta,
        candidate_word_delta=candidate_word_delta,
        char_regression=abs(candidate_char_delta) - abs(baseline_char_delta),
        word_regression=abs(candidate_word_delta) - abs(baseline_word_delta),
    )


def baseline_candidate_deltas(
    baseline: dict[str, int], candidate: dict[str, int]
) -> MetricDeltas:
    """Compare against baseline when no independent reference is supplied.

    A no-reference run is a regression gate, not an accuracy gate: losing
    characters/words fails, while gaining output is an improvement and must
    not be rejected merely because it differs from baseline.
    """
    candidate_char_delta = candidate["chars"] - baseline["chars"]
    candidate_word_delta = candidate["words"] - baseline["words"]
    return MetricDeltas(
        baseline_char_delta=0,
        baseline_word_delta=0,
        candidate_char_delta=candidate_char_delta,
        candidate_word_delta=candidate_word_delta,
        char_regression=max(0, -candidate_char_delta),
        word_regression=max(0, -candidate_word_delta),
    )


def build_document_report(
    config: HarnessConfig,
    document: CorpusDocument,
    baseline: dict[str, Any],
    candidate: dict[str, Any],
) -> dict[str, Any]:
    reference = load_reference(config, document, baseline)
    thresholds = thresholds_for_document(config, document)
    failures: list[str] = []

    expected_errors: list[str] = []
    for side, prediction in (("baseline", baseline), ("candidate", candidate)):
        failure = failure_for_prediction(side, prediction, adversarial=document.adversarial)
        if failure:
            failures.append(failure)
        elif document.adversarial and prediction["status"] == "error":
            expected_errors.append(f"{side}_expected_parser_error")
    expected_parser_outcome = (
        document.adversarial
        and baseline["status"] == "error"
        and candidate["status"] == "error"
    )
    if reference["error"] and not expected_parser_outcome:
        failures.append(reference["error"])
    if document.adversarial:
        baseline_error = baseline["status"] == "error"
        candidate_error = candidate["status"] == "error"
        if baseline_error != candidate_error:
            failures.append("adversarial_outcome_mismatch")

    deltas: MetricDeltas | None = None
    if reference["metrics"] and baseline["metrics"] and candidate["metrics"]:
        if config.reference_dir is None:
            deltas = baseline_candidate_deltas(baseline["metrics"], candidate["metrics"])
        else:
            deltas = metric_deltas(reference["metrics"], baseline["metrics"], candidate["metrics"])
        if deltas.char_regression > thresholds.max_char_regression:
            failures.append(
                f"char_regression_threshold_exceeded:{deltas.char_regression}>{thresholds.max_char_regression}"
            )
        if deltas.word_regression > thresholds.max_word_regression:
            failures.append(
                f"word_regression_threshold_exceeded:{deltas.word_regression}>{thresholds.max_word_regression}"
            )
        if (
            thresholds.max_candidate_char_delta is not None
            and abs(deltas.candidate_char_delta) > thresholds.max_candidate_char_delta
        ):
            failures.append(
                "candidate_char_delta_threshold_exceeded:"
                f"{abs(deltas.candidate_char_delta)}>{thresholds.max_candidate_char_delta}"
            )
        if (
            thresholds.max_candidate_word_delta is not None
            and abs(deltas.candidate_word_delta) > thresholds.max_candidate_word_delta
        ):
            failures.append(
                "candidate_word_delta_threshold_exceeded:"
                f"{abs(deltas.candidate_word_delta)}>{thresholds.max_candidate_word_delta}"
            )

    return {
        "id": document.id,
        "filename": document.filename,
        "path": str(document.path),
        "adversarial": document.adversarial,
        "expected_sha256": document.expected_sha256,
        "actual_sha256": document.actual_sha256,
        "reference": reference,
        "baseline": baseline,
        "candidate": candidate,
        "deltas": dataclasses.asdict(deltas) if deltas else None,
        "thresholds": dataclasses.asdict(thresholds),
        "status": "failed" if failures else "passed",
        "failures": failures,
        "expected_errors": expected_errors,
    }


def command_mentions_document(command: str) -> bool:
    """Require an explicit per-document input for non-test harness runs."""
    tokens = ("{pdf}", "{pdf_basename}", "{doc_id}", "PDF_PATH", "PDF_BASENAME", "DOC_ID")
    return any(token in command for token in tokens)


def command_mentions_side(command: str) -> bool:
    return any(token in command for token in ("{side}", "RZN_PAIRED_SIDE"))


def command_signature(command: str) -> tuple[str, ...]:
    """Normalize only the harness-managed output sink before comparing commands."""
    try:
        tokens = shlex.split(command)
    except ValueError:
        return (command.strip(),)
    return tuple(
        token
        for token in tokens
        if token not in ("$RZN_PAIRED_OUTPUT", "${RZN_PAIRED_OUTPUT}", "{output}")
    )


def prediction_is_crash(prediction: dict[str, Any]) -> bool:
    if prediction.get("status") != "error":
        return False
    exit_code = prediction.get("exit_code")
    if isinstance(exit_code, int) and (exit_code < 0 or 128 <= exit_code <= 192):
        return True
    stderr = (prediction.get("stderr_excerpt") or "").lower()
    return any(marker in stderr for marker in FATAL_MARKERS)


def summarize(documents: list[dict[str, Any]]) -> dict[str, int]:
    totals = {
        "passed": 0,
        "failed": 0,
        "baseline_errors": 0,
        "candidate_errors": 0,
        "missing_predictions": 0,
        "threshold_failures": 0,
        "adversarial_failures": 0,
        "expected_parser_errors": 0,
    }
    for document in documents:
        totals["passed" if document["status"] == "passed" else "failed"] += 1
        failures = document["failures"]
        if any(failure.startswith("baseline_") for failure in failures):
            totals["baseline_errors"] += 1
        if any(failure.startswith("candidate_") for failure in failures):
            totals["candidate_errors"] += 1
        if any("missing_prediction" in failure for failure in failures):
            totals["missing_predictions"] += 1
        if any("threshold_exceeded" in failure for failure in failures):
            totals["threshold_failures"] += 1
        if document["adversarial"] and document["status"] != "passed":
            totals["adversarial_failures"] += 1
        totals["expected_parser_errors"] += len(document.get("expected_errors", []))
    return totals


def run_harness(config: HarnessConfig) -> dict[str, Any]:
    documents = load_corpus(config)
    if config.require_document_bound_commands:
        for side, command in (("baseline", config.baseline_cmd), ("candidate", config.candidate_cmd)):
            if not command_mentions_document(command):
                raise ValueError(
                    f"{side} command is not document-bound; use {{pdf}}/{{doc_id}} or "
                    "$PDF_PATH/$DOC_ID so the extractor actually receives each corpus PDF"
                )
        if (
            (
                config.baseline_rev.strip() == config.candidate_rev.strip()
                or command_signature(config.baseline_cmd)
                == command_signature(config.candidate_cmd)
            )
            and not command_mentions_side(config.baseline_cmd)
        ):
            raise ValueError(
                "baseline and candidate commands are identical or otherwise indistinguishable; use distinct "
                "revisions and commands/artifacts or branch explicitly on RZN_PAIRED_SIDE"
            )
    if config.require_adversarial_fixtures and not any(document.adversarial for document in documents):
        raise ValueError("at least one adversarial PDF fixture is required")
    reports: list[dict[str, Any]] = []
    for document in documents:
        suffix = " [adversarial]" if document.adversarial else ""
        print(f"[paired-regression] {document.relative_path}{suffix}", file=sys.stderr)
        baseline = run_extractor(
            side="baseline",
            template=config.baseline_cmd,
            document=document,
            timeout_ms=config.timeout_ms,
        )
        candidate = run_extractor(
            side="candidate",
            template=config.candidate_cmd,
            document=document,
            timeout_ms=config.timeout_ms,
        )
        reports.append(build_document_report(config, document, baseline, candidate))

    totals = summarize(reports)
    return {
        "schema_version": 1,
        "generated_at_unix_s": int(time.time()),
        "passed": totals["failed"] == 0,
        "corpus": {
            "dir": str(config.corpus_dir),
            "sha256_manifest": str(config.corpus_sha256) if config.corpus_sha256 else None,
            "revision_fingerprint": corpus_fingerprint(documents),
            "reference_source": str(config.reference_dir)
            if config.reference_dir
            else f"baseline-output:{config.baseline_rev}",
            "document_count": len(documents),
            "adversarial_document_count": sum(1 for document in documents if document.adversarial),
        },
        "baseline": {"revision": config.baseline_rev, "command": config.baseline_cmd},
        "candidate": {"revision": config.candidate_rev, "command": config.candidate_cmd},
        "execution_contract": {
            "document_bound_commands": not config.require_document_bound_commands
            or all(command_mentions_document(command) for command in (config.baseline_cmd, config.candidate_cmd)),
            "distinct_command_templates": config.baseline_cmd.strip() != config.candidate_cmd.strip()
            or command_mentions_side(config.baseline_cmd),
            "adversarial_fixtures_required": config.require_adversarial_fixtures,
            "adversarial_fixtures_present": any(document.adversarial for document in documents),
        },
        "thresholds": {
            "source": str(config.thresholds_source) if config.thresholds_source else "cli-defaults",
            "default": dataclasses.asdict(config.thresholds_default),
            "documents": config.thresholds_documents,
        },
        "totals": totals,
        "documents": reports,
    }


def metric_cell(metrics: dict[str, int] | None) -> str:
    return "-" if not metrics else f"{metrics['chars']}/{metrics['words']}"


def render_markdown(report: dict[str, Any]) -> str:
    lines = [
        f"# Paired PDF regression — {report['baseline']['revision']} vs {report['candidate']['revision']}",
        "",
        f"- Result: **{'PASS' if report['passed'] else 'FAIL'}**",
        f"- Corpus: `{report['corpus']['dir']}`",
        f"- Corpus fingerprint: `{report['corpus']['revision_fingerprint']}`",
        f"- Reference source: `{report['corpus']['reference_source']}`",
        (
            f"- Totals: {report['totals']['passed']} passed, {report['totals']['failed']} failed; "
            f"missing predictions {report['totals']['missing_predictions']}; "
            f"threshold failures {report['totals']['threshold_failures']}; "
            f"adversarial failures {report['totals']['adversarial_failures']}; "
            f"expected parser errors {report['totals']['expected_parser_errors']}"
        ),
        "",
        "| document | adv | baseline c/w | candidate c/w | ref c/w | candidate Δ c/w | regression c/w | threshold c/w | status | failures |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---|---|",
    ]
    for document in report["documents"]:
        deltas = document["deltas"]
        candidate_delta = (
            "-"
            if not deltas
            else f"{deltas['candidate_char_delta']}/{deltas['candidate_word_delta']}"
        )
        regression = "-" if not deltas else f"{deltas['char_regression']}/{deltas['word_regression']}"
        thresholds = document["thresholds"]
        row_failures = list(document["failures"])
        row_failures.extend(document.get("expected_errors", []))
        failures = "<br>".join(row_failures) if row_failures else "-"
        lines.append(
            "| `{filename}` | {adv} | {baseline} | {candidate} | {reference} | {candidate_delta} | "
            "{regression} | {threshold} | {status} | {failures} |".format(
                filename=document["filename"],
                adv="yes" if document["adversarial"] else "no",
                baseline=metric_cell(document["baseline"]["metrics"]),
                candidate=metric_cell(document["candidate"]["metrics"]),
                reference=metric_cell(document["reference"]["metrics"]),
                candidate_delta=candidate_delta,
                regression=regression,
                threshold=f"{thresholds['max_char_regression']}/{thresholds['max_word_regression']}",
                status=document["status"],
                failures=failures,
            )
        )
    return "\n".join(lines) + "\n"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        formatter_class=argparse.RawDescriptionHelpFormatter,
        description="Run paired baseline/candidate PDF regression over one corpus revision.",
        epilog=textwrap.dedent(
            """\
            Command templates run through the shell and receive:
              PDF_PATH, PDF_BASENAME, DOC_ID, RZN_PAIRED_SIDE, RZN_PAIRED_OUTPUT

            Shell-quoted placeholders are also supported:
              {pdf}, {pdf_basename}, {doc_id}, {side}, {output}

            Example against the existing benchmark corpus:
              python3 scripts/paired_regression.py \\
                --corpus-dir benchmarks/corpus \\
                --corpus-sha256 benchmarks/corpus.sha256 \\
                --baseline-rev main \\
                --baseline-cmd 'cargo run --quiet --manifest-path ../rag_pdf_extract-main/Cargo.toml --example extract_markdown -- "$PDF_PATH"' \\
                --candidate-rev HEAD \\
                --candidate-cmd 'cargo run --quiet --example extract_markdown -- "$PDF_PATH"' \\
                --output target/paired-regression/report.json \\
                --markdown-output target/paired-regression/report.md
            """
        ),
    )
    parser.add_argument("--self-test", action="store_true", help="run embedded unit tests")
    parser.add_argument("--corpus-dir", type=pathlib.Path)
    parser.add_argument("--corpus-sha256", type=pathlib.Path)
    parser.add_argument("--reference-dir", type=pathlib.Path)
    parser.add_argument("--baseline-rev")
    parser.add_argument("--baseline-cmd")
    parser.add_argument("--candidate-rev")
    parser.add_argument("--candidate-cmd")
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--markdown-output", type=pathlib.Path)
    parser.add_argument("--thresholds", type=pathlib.Path)
    parser.add_argument("--default-max-char-regression", type=int, default=0)
    parser.add_argument("--default-max-word-regression", type=int, default=0)
    parser.add_argument("--default-max-candidate-char-delta", type=int)
    parser.add_argument("--default-max-candidate-word-delta", type=int)
    parser.add_argument("--adversarial-dir", action="append", type=pathlib.Path, default=[])
    parser.add_argument("--adversarial-fixture", action="append", type=pathlib.Path, default=[])
    parser.add_argument(
        "--require-adversarial",
        action="store_true",
        help="fail unless the run includes at least one adversarial PDF fixture",
    )
    parser.add_argument(
        "--require-document-bound-commands",
        action="store_true",
        help="fail unless both commands reference the current PDF/document environment",
    )
    parser.add_argument("--timeout-ms", type=int, default=DEFAULT_TIMEOUT_MS)
    return parser


def config_from_args(args: argparse.Namespace, parser: argparse.ArgumentParser) -> HarnessConfig:
    required = [
        "corpus_dir",
        "baseline_rev",
        "baseline_cmd",
        "candidate_rev",
        "candidate_cmd",
        "output",
    ]
    missing = [f"--{name.replace('_', '-')}" for name in required if getattr(args, name) is None]
    if missing:
        parser.error("missing required arguments: " + ", ".join(missing))

    cli_default = Thresholds(
        max_char_regression=args.default_max_char_regression,
        max_word_regression=args.default_max_word_regression,
        max_candidate_char_delta=args.default_max_candidate_char_delta,
        max_candidate_word_delta=args.default_max_candidate_word_delta,
    )
    threshold_default, threshold_documents, _ = load_thresholds(args.thresholds, cli_default)
    return HarnessConfig(
        corpus_dir=args.corpus_dir,
        corpus_sha256=args.corpus_sha256,
        reference_dir=args.reference_dir,
        baseline_rev=args.baseline_rev,
        baseline_cmd=args.baseline_cmd,
        candidate_rev=args.candidate_rev,
        candidate_cmd=args.candidate_cmd,
        output=args.output,
        markdown_output=args.markdown_output,
        thresholds_source=args.thresholds,
        thresholds_default=threshold_default,
        thresholds_documents=threshold_documents,
        adversarial_dirs=tuple(args.adversarial_dir),
        adversarial_fixtures=tuple(args.adversarial_fixture),
        timeout_ms=args.timeout_ms,
        require_document_bound_commands=args.require_document_bound_commands,
        require_adversarial_fixtures=args.require_adversarial,
    )


def write_report(config: HarnessConfig, report: dict[str, Any]) -> None:
    config.output.parent.mkdir(parents=True, exist_ok=True)
    config.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if config.markdown_output:
        config.markdown_output.parent.mkdir(parents=True, exist_ok=True)
        config.markdown_output.write_text(render_markdown(report), encoding="utf-8")


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(PairedRegressionTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.self_test:
        return run_self_tests()
    config = config_from_args(args, parser)
    try:
        report = run_harness(config)
    except (OSError, ValueError) as error:
        print(f"paired regression: configuration error: {error}", file=sys.stderr)
        return 2
    write_report(config, report)
    print(
        "paired regression: {status}; {passed} passed, {failed} failed; report: {report_path}".format(
            status="PASS" if report["passed"] else "FAIL",
            passed=report["totals"]["passed"],
            failed=report["totals"]["failed"],
            report_path=config.output,
        )
    )
    return 0 if report["passed"] else 1


class TempWorkspace:
    def __init__(self, name: str) -> None:
        self.root = pathlib.Path(tempfile.mkdtemp(prefix=f"paired-regression-{name}-"))

    @property
    def corpus_dir(self) -> pathlib.Path:
        return self.root / "corpus"

    @property
    def reference_dir(self) -> pathlib.Path:
        return self.root / "refs"

    def write_doc(self, name: str, reference: str | None) -> pathlib.Path:
        self.corpus_dir.mkdir(parents=True, exist_ok=True)
        path = self.corpus_dir / name
        path.write_bytes(b"%PDF-1.7\n% tiny command-broker fixture\n")
        if reference is not None:
            self.reference_dir.mkdir(parents=True, exist_ok=True)
            (self.reference_dir / name).with_suffix(".txt").write_text(reference, encoding="utf-8")
        return path

    def write_script(self, body: str) -> pathlib.Path:
        path = self.root / "extractor.sh"
        path.write_text(body, encoding="utf-8")
        return path

    def cleanup(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)


class PairedRegressionTests(unittest.TestCase):
    def base_config(self, workspace: TempWorkspace, script: pathlib.Path) -> HarnessConfig:
        command = f"sh {shlex.quote(str(script))}"
        return HarnessConfig(
            corpus_dir=workspace.corpus_dir,
            reference_dir=workspace.reference_dir,
            baseline_rev="base-rev",
            baseline_cmd=command,
            candidate_rev="candidate-rev",
            candidate_cmd=command,
            output=workspace.root / "report.json",
            timeout_ms=5_000,
        )

    def test_missing_candidate_prediction_is_a_failure(self) -> None:
        workspace = TempWorkspace("missing")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("missing.pdf", "alpha beta")
        script = workspace.write_script(
            """case "$RZN_PAIRED_SIDE:$PDF_BASENAME" in
baseline:missing.pdf) printf 'alpha beta\\n' ;;
candidate:missing.pdf) : ;;
*) printf 'alpha beta\\n' ;;
esac
"""
        )

        report = run_harness(self.base_config(workspace, script))

        self.assertFalse(report["passed"])
        self.assertEqual(report["documents"][0]["candidate"]["status"], "missing_prediction")
        self.assertIn("candidate_missing_prediction", report["documents"][0]["failures"])
        self.assertEqual(report["totals"]["missing_predictions"], 1)

    def test_per_document_threshold_failure_is_not_a_skip(self) -> None:
        workspace = TempWorkspace("threshold")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("regress.pdf", "alpha beta gamma delta")
        script = workspace.write_script(
            """case "$RZN_PAIRED_SIDE:$PDF_BASENAME" in
baseline:regress.pdf) printf 'alpha beta gamma delta\\n' ;;
candidate:regress.pdf) printf 'alpha beta\\n' ;;
*) printf 'alpha beta gamma delta\\n' ;;
esac
"""
        )
        config = dataclasses.replace(
            self.base_config(workspace, script),
            thresholds_documents={
                "regress.pdf": {
                    "max_char_regression": 0,
                    "max_word_regression": 0,
                }
            },
        )

        report = run_harness(config)

        self.assertFalse(report["passed"])
        self.assertEqual(report["documents"][0]["status"], "failed")
        self.assertTrue(
            any(
                failure.startswith("char_regression_threshold_exceeded:")
                for failure in report["documents"][0]["failures"]
            )
        )
        self.assertTrue(
            any(
                failure.startswith("word_regression_threshold_exceeded:")
                for failure in report["documents"][0]["failures"]
            )
        )
        self.assertEqual(report["totals"]["threshold_failures"], 1)

    def test_adversarial_command_errors_are_recorded_and_later_docs_still_run(self) -> None:
        workspace = TempWorkspace("adversarial")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("ok.pdf", None)
        adversarial_dir = workspace.root / "adversarial"
        adversarial_dir.mkdir()
        adversarial = adversarial_dir / "deep-nesting-bomb.pdf"
        adversarial.write_bytes(b"%PDF-1.7\n% malformed adversarial fixture\n")
        script = workspace.write_script(
            """case "$RZN_PAIRED_SIDE:$PDF_BASENAME" in
candidate:deep-nesting-bomb.pdf) echo 'simulated parser panic converted to process error' >&2; exit 22 ;;
*) printf 'alpha beta gamma\\n' ;;
esac
"""
        )
        config = dataclasses.replace(
            self.base_config(workspace, script),
            reference_dir=None,
            adversarial_fixtures=(adversarial,),
        )

        report = run_harness(config)

        self.assertFalse(report["passed"])
        self.assertEqual([doc["filename"] for doc in report["documents"]], ["ok.pdf", "deep-nesting-bomb.pdf"])
        self.assertEqual(report["documents"][0]["status"], "passed")
        self.assertTrue(report["documents"][1]["adversarial"])
        self.assertEqual(report["documents"][1]["candidate"]["status"], "error")
        self.assertIn("candidate_crash", report["documents"][1]["failures"])
        self.assertEqual(report["totals"]["adversarial_failures"], 1)

    def test_adversarial_signal_exit_is_a_crash_failure(self) -> None:
        workspace = TempWorkspace("adversarial-crash")
        self.addCleanup(workspace.cleanup)
        workspace.corpus_dir.mkdir(parents=True)
        adversarial_dir = workspace.root / "adversarial"
        adversarial_dir.mkdir()
        adversarial = adversarial_dir / "deep-nesting-bomb.pdf"
        adversarial.write_bytes(b"%PDF-1.7\n% adversarial fixture\n")
        script = workspace.write_script(
            """case "$RZN_PAIRED_SIDE" in
candidate) kill -SEGV $$ ;;
*) printf 'alpha beta gamma\\n' ;;
esac
"""
        )
        config = dataclasses.replace(
            self.base_config(workspace, script),
            reference_dir=None,
            adversarial_fixtures=(adversarial,),
        )

        report = run_harness(config)

        self.assertFalse(report["passed"])
        self.assertIn("candidate_crash", report["documents"][0]["failures"])

    def test_cli_uses_explicit_commands_revisions_and_writes_report(self) -> None:
        workspace = TempWorkspace("cli")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("cli.pdf", "alpha beta")
        script = workspace.write_script(
            """case "$RZN_PAIRED_SIDE:$PDF_BASENAME" in
baseline:cli.pdf) printf 'alpha beta\\n' ;;
candidate:cli.pdf) : ;;
*) printf 'alpha beta\\n' ;;
esac
"""
        )
        output = workspace.root / "out/report.json"
        markdown = workspace.root / "out/report.md"

        with contextlib.redirect_stdout(io.StringIO()):
            exit_code = main(
                [
                    "--corpus-dir",
                    str(workspace.corpus_dir),
                    "--reference-dir",
                    str(workspace.reference_dir),
                    "--baseline-rev",
                    "baseline-sha",
                    "--baseline-cmd",
                    f"sh {shlex.quote(str(script))}",
                    "--candidate-rev",
                    "candidate-sha",
                    "--candidate-cmd",
                    f"sh {shlex.quote(str(script))}",
                    "--output",
                    str(output),
                    "--markdown-output",
                    str(markdown),
                ]
            )

        self.assertEqual(exit_code, 1)
        report = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(report["baseline"]["revision"], "baseline-sha")
        self.assertEqual(report["candidate"]["revision"], "candidate-sha")
        self.assertEqual(report["documents"][0]["candidate"]["status"], "missing_prediction")
        self.assertTrue(markdown.exists())

    def test_strict_contract_rejects_static_commands(self) -> None:
        workspace = TempWorkspace("strict-contract")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("contract.pdf", "alpha beta")
        script = workspace.write_script("printf 'alpha beta\\n'\n")
        config = dataclasses.replace(
            self.base_config(workspace, script),
            require_document_bound_commands=True,
        )

        with self.assertRaisesRegex(ValueError, "not document-bound"):
            run_harness(config)

    def test_strict_contract_rejects_identical_document_bound_commands(self) -> None:
        workspace = TempWorkspace("strict-identical")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("contract.pdf", "alpha beta")
        script = workspace.write_script("printf 'alpha beta\\n'\n")
        command = f"sh {shlex.quote(str(script))} \"$PDF_PATH\""
        config = dataclasses.replace(
            self.base_config(workspace, script),
            baseline_cmd=command,
            candidate_cmd=command,
            require_document_bound_commands=True,
        )

        with self.assertRaisesRegex(ValueError, "commands are identical"):
            run_harness(config)

    def test_strict_contract_rejects_output_only_command_difference(self) -> None:
        workspace = TempWorkspace("strict-output-only")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("contract.pdf", "alpha beta")
        script = workspace.write_script("printf 'alpha beta\\n'\n")
        baseline = f"sh {shlex.quote(str(script))} \"$PDF_PATH\""
        candidate = baseline + ' "$RZN_PAIRED_OUTPUT"'
        config = dataclasses.replace(
            self.base_config(workspace, script),
            baseline_cmd=baseline,
            candidate_cmd=candidate,
            require_document_bound_commands=True,
        )

        with self.assertRaisesRegex(ValueError, "indistinguishable"):
            run_harness(config)

    def test_no_reference_mode_allows_candidate_improvements(self) -> None:
        workspace = TempWorkspace("improvement")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("improvement.pdf", None)
        baseline = workspace.root / "baseline.sh"
        baseline.write_text("printf 'alpha\\n'\n", encoding="utf-8")
        candidate = workspace.root / "candidate.sh"
        candidate.write_text("printf 'alpha beta gamma\\n'\n", encoding="utf-8")
        config = dataclasses.replace(
            self.base_config(workspace, baseline),
            reference_dir=None,
            baseline_cmd=f"sh {shlex.quote(str(baseline))}",
            candidate_cmd=f"sh {shlex.quote(str(candidate))}",
        )

        report = run_harness(config)

        self.assertTrue(report["passed"])
        self.assertEqual(report["documents"][0]["deltas"]["char_regression"], 0)
        self.assertGreater(report["documents"][0]["deltas"]["candidate_char_delta"], 0)

    def test_strict_contract_requires_adversarial_fixture(self) -> None:
        workspace = TempWorkspace("strict-adversarial")
        self.addCleanup(workspace.cleanup)
        workspace.write_doc("contract.pdf", "alpha beta")
        script = workspace.write_script("printf 'alpha beta\\n'\n")
        config = dataclasses.replace(
            self.base_config(workspace, script),
            require_adversarial_fixtures=True,
        )

        with self.assertRaisesRegex(ValueError, "adversarial PDF fixture"):
            run_harness(config)


if __name__ == "__main__":
    sys.exit(main())
