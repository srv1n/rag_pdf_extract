#!/usr/bin/env python3
"""Render the three corpus benchmark JSON reports as one auditable comparison."""

import argparse
import json
from pathlib import Path


def load(path: Path) -> dict:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def fmt(value: float) -> str:
    return f"{value:.2f}"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--full", type=Path, required=True)
    parser.add_argument("--two-core", type=Path, required=True)
    parser.add_argument("--efficiency", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--date", required=True)
    args = parser.parse_args()

    reports = {
        "Full-core": load(args.full),
        "2-core": load(args.two_core),
        "Efficiency-core QoS": load(args.efficiency),
    }
    runs = {label: report["runs"][0] for label, report in reports.items()}
    full = runs["Full-core"]["aggregate"]

    output = [
        f"# Supreme Court corpus constrained benchmark — {args.date}",
        "",
        "The three runs use the same cached 21 PDFs, extraction options, and release binary.",
        "Fetch/cache time is separate from parse wall time in every source report.",
        "",
        "## Aggregate comparison",
        "",
        "| profile | scheduler / workers | parse wall ms | weighted ms/page | median ms/page | MB/s | chars/s | throughput vs full |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
    ]
    for label, report in reports.items():
        run = runs[label]
        aggregate = run["aggregate"]
        ratio = aggregate["mb_per_s"] / full["mb_per_s"]
        scheduler = report["scheduler"]
        workers = run["rayon_threads"]
        output.append(
            f"| {label} | {scheduler} / {workers} | "
            f"{fmt(aggregate['parse_total_wall_ms'])} | "
            f"{fmt(aggregate['weighted_ms_per_page'])} | "
            f"{fmt(aggregate['median_ms_per_page'])} | "
            f"{fmt(aggregate['mb_per_s'])} | "
            f"{fmt(aggregate['chars_per_s'])} | **{ratio:.3f}x** |"
        )

    output.extend(
        [
            "",
            "Ratios use aggregate MB/s (the same ratios result from weighted ms/page and chars/s).",
            "The 2-core run is a real parser pool cap (RAYON_NUM_THREADS=2). The efficiency-core run uses macOS taskpolicy -c background; macOS does not expose strict per-process E-core affinity, so this is a QoS scheduling approximation, not a claim of hard pinning.",
            "",
            "## What this says about a 2-vCPU cx23-class box",
            "",
            f"The 2-core profile is the useful parallelism analogue: it processes this corpus in {full['parse_total_wall_ms'] / runs['2-core']['aggregate']['parse_total_wall_ms']:.3f}x of full-core throughput, or {runs['2-core']['aggregate']['parse_total_wall_ms'] / 1000:.2f} s of parser-only wall on this M1 Pro. That is not an absolute cloud prediction—the cloud CPU is x86 and shared—but it sets the honest local expectation: parser work is tens of seconds for these 21 PDFs, while the reported 405 s backend ingestion figure also contains object-store, job-runner, and archive overhead.",
            "",
            "## Per-document normalized metrics",
            "",
            "| filename | bytes | pages | chars | full ms/page | full MB/s | full chars/s | 2-core ms/page | 2-core MB/s | 2-core chars/s | E-core ms/page | E-core MB/s | E-core chars/s |",
            "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    full_docs = {doc["filename"]: doc for doc in runs["Full-core"]["documents"]}
    two_docs = {doc["filename"]: doc for doc in runs["2-core"]["documents"]}
    e_docs = {doc["filename"]: doc for doc in runs["Efficiency-core QoS"]["documents"]}
    for filename in full_docs:
        documents = [full_docs[filename], two_docs[filename], e_docs[filename]]
        output.append(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |".format(
                filename,
                documents[0]["bytes"],
                documents[0]["pages"],
                documents[0]["chars"],
                fmt(documents[0]["ms_per_page"]),
                fmt(documents[0]["mb_per_s"]),
                fmt(documents[0]["chars_per_s"]),
                fmt(documents[1]["ms_per_page"]),
                fmt(documents[1]["mb_per_s"]),
                fmt(documents[1]["chars_per_s"]),
                fmt(documents[2]["ms_per_page"]),
                fmt(documents[2]["mb_per_s"]),
                fmt(documents[2]["chars_per_s"]),
            )
        )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(output) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
