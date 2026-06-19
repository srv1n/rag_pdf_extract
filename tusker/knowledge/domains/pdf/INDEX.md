---
schema: "tusker.domain/v7"
kind: "domain"
id: "pdf"
project: "rag_pdf_extract"
title: "Pdf"
status: "current"
summary: "Durable source of truth for Pdf."
source_of_truth:
  - "knowledge/domains/pdf/CANON.md"
canonical_files:
  - "INDEX.md"
  - "CANON.md"
created_at: "2026-05-29T17:58:14Z"
updated_at: "2026-05-29T17:58:14Z"
state_rev: "sha256:5ec1c2e148b5bedea2f01a7d32079a010abfcc974ba86a8c3ef8efeb33c0028e"
---

# Pdf

## Summary

Durable source of truth for Pdf.

## Read This When

- You need current source-of-truth context for pdf.
- You are changing behavior owned by this domain.

## Canonical Files

- CANON.md - current durable truth.
- INDEX.md - domain map and routing hints.

## Runbooks

- _None yet._

## Interfaces

- _No stable interfaces declared yet._

## Invariants

- Keep durable truth in CANON.md.
- Put procedural guidance in runbooks/.

## Sources

- Raw external input belongs in sources/. Do not treat root docs/ or site output as canonical V7 knowledge.

## Glossary

- See glossary.md.

## Current Work

- _No current work linked._
