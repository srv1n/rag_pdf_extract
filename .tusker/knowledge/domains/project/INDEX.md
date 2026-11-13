---
schema: "tusker.domain/v7"
kind: "domain"
id: "project"
project: "rag_pdf_extract"
title: "Project"
status: "current"
summary: "Durable project knowledge."
capsule:
  what: "Domain index for Project; routes agents to canon and owned knowledge files."
  use_when: "Use when a task touches project behavior or needs the domain reading order."
  skip_when: "Skip when another domain is narrower or task proof/gates are the target."
source_of_truth:
  - "knowledge/domains/project/CANON.md"
canonical_files:
  - "INDEX.md"
  - "CANON.md"
created_at: "2026-11-13T02:27:50Z"
updated_at: "2026-11-13T02:27:50Z"
state_rev: "sha256:ae5e08477b90a9dcb60b17e4ff5d9c528db7122713790e8c5be6c12fbeb0a830"
---

# Project

## Summary

Durable project knowledge.

## Read This When

- You need current source-of-truth context for project.
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

- Raw external input belongs in sources/. Do not treat root docs/ or site output as domain canon.

## Glossary

- See glossary.md.

## Current Work

- _No current work linked._
