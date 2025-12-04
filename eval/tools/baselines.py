#!/usr/bin/env python3
"""
Baseline extraction tools for comparison.

Supports:
- ours: Our Rust-based extraction (extract_markdown)
- markitdown: Microsoft's MarkItDown (PyMuPDF/pdfminer-based)
- gemini: Google Gemini API for OCR-based extraction
- claude: Anthropic Claude API for OCR-based extraction

Usage:
    from tools.baselines import extract_with_tool, AVAILABLE_TOOLS

    markdown = extract_with_tool("markitdown", pdf_path)
"""

import os
import sys
import subprocess
import tempfile
import base64
from pathlib import Path
from typing import Optional

# Project paths
EVAL_DIR = Path(__file__).parent.parent
PROJECT_ROOT = EVAL_DIR.parent
EXTRACT_BINARY = PROJECT_ROOT / "target" / "release" / "examples" / "extract_markdown"

# Available tools
AVAILABLE_TOOLS = {
    "ours": "Our Rust extraction (non-OCR)",
    "markitdown": "Microsoft MarkItDown (non-OCR, uses PyMuPDF)",
    "gemini": "Google Gemini API (OCR-capable)",
    "claude": "Anthropic Claude API (OCR-capable)",
}


# =============================================================================
# Our Extraction
# =============================================================================

def extract_with_ours(pdf_path: Path) -> str:
    """Extract using our Rust-based tool."""
    if not EXTRACT_BINARY.exists():
        # Build if needed
        result = subprocess.run(
            ["cargo", "build", "--release", "--example", "extract_markdown"],
            cwd=PROJECT_ROOT,
            capture_output=True,
            text=True
        )
        if result.returncode != 0:
            raise RuntimeError(f"Build failed: {result.stderr}")

    result = subprocess.run(
        [str(EXTRACT_BINARY), str(pdf_path), "--max-tokens", "500"],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT
    )

    if result.returncode != 0:
        raise RuntimeError(f"Extraction failed: {result.stderr}")

    return result.stdout


# =============================================================================
# MarkItDown (Microsoft)
# =============================================================================

def extract_with_markitdown(pdf_path: Path) -> str:
    """
    Extract using Microsoft's MarkItDown.
    Install: pip install markitdown
    """
    try:
        from markitdown import MarkItDown
    except ImportError:
        raise ImportError(
            "MarkItDown not installed. Run: pip install markitdown"
        )

    md = MarkItDown()
    result = md.convert(str(pdf_path))
    return result.text_content


# =============================================================================
# Gemini API (OCR-capable)
# =============================================================================

def extract_with_gemini(pdf_path: Path) -> str:
    """
    Extract using Google Gemini API with vision capability.
    Requires: GEMINI_API_KEY environment variable
    Install: pip install google-generativeai
    """
    api_key = os.environ.get("GEMINI_API_KEY")
    if not api_key:
        raise ValueError("GEMINI_API_KEY environment variable not set")

    try:
        import google.generativeai as genai
    except ImportError:
        raise ImportError(
            "google-generativeai not installed. Run: pip install google-generativeai"
        )

    genai.configure(api_key=api_key)

    # Upload PDF file
    uploaded_file = genai.upload_file(str(pdf_path))

    # Use Gemini to extract text
    model = genai.GenerativeModel("gemini-2.0-flash")

    prompt = """Extract all text from this PDF document and format it as clean markdown.

Rules:
- Preserve the document structure with proper heading levels (# ## ###)
- Keep paragraphs intact
- Preserve lists and tables if present
- Do not summarize - extract the COMPLETE text
- Do not add any commentary or explanation
- Output ONLY the extracted markdown

Begin extraction:"""

    response = model.generate_content([prompt, uploaded_file])

    return response.text


# =============================================================================
# Claude API (OCR-capable)
# =============================================================================

def extract_with_claude(pdf_path: Path) -> str:
    """
    Extract using Anthropic Claude API with vision capability.
    Requires: ANTHROPIC_API_KEY environment variable
    Install: pip install anthropic
    """
    api_key = os.environ.get("ANTHROPIC_API_KEY")
    if not api_key:
        raise ValueError("ANTHROPIC_API_KEY environment variable not set")

    try:
        import anthropic
    except ImportError:
        raise ImportError(
            "anthropic not installed. Run: pip install anthropic"
        )

    # Claude needs PDF as base64
    with open(pdf_path, "rb") as f:
        pdf_data = base64.standard_b64encode(f.read()).decode("utf-8")

    client = anthropic.Anthropic(api_key=api_key)

    message = client.messages.create(
        model="claude-sonnet-4-20250514",
        max_tokens=8192,
        messages=[
            {
                "role": "user",
                "content": [
                    {
                        "type": "document",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": pdf_data,
                        },
                    },
                    {
                        "type": "text",
                        "text": """Extract all text from this PDF document and format it as clean markdown.

Rules:
- Preserve the document structure with proper heading levels (# ## ###)
- Keep paragraphs intact
- Preserve lists and tables if present
- Do not summarize - extract the COMPLETE text
- Do not add any commentary or explanation
- Output ONLY the extracted markdown

Begin extraction:"""
                    }
                ],
            }
        ],
    )

    return message.content[0].text


# =============================================================================
# Unified Interface
# =============================================================================

EXTRACTORS = {
    "ours": extract_with_ours,
    "markitdown": extract_with_markitdown,
    "gemini": extract_with_gemini,
    "claude": extract_with_claude,
}


def extract_with_tool(tool: str, pdf_path: Path) -> str:
    """
    Extract PDF using specified tool.

    Args:
        tool: One of 'ours', 'markitdown', 'gemini', 'claude'
        pdf_path: Path to PDF file

    Returns:
        Extracted markdown text
    """
    if tool not in EXTRACTORS:
        raise ValueError(f"Unknown tool: {tool}. Available: {list(EXTRACTORS.keys())}")

    extractor = EXTRACTORS[tool]
    return extractor(pdf_path)


def check_tool_available(tool: str) -> tuple[bool, str]:
    """
    Check if a tool is available and return status.

    Returns:
        (is_available, message)
    """
    if tool == "ours":
        if EXTRACT_BINARY.exists():
            return True, "Binary exists"
        # Try to build
        try:
            extract_with_ours(Path("/dev/null"))  # Will fail but build first
        except:
            pass
        return EXTRACT_BINARY.exists(), "Needs cargo build"

    elif tool == "markitdown":
        try:
            from markitdown import MarkItDown
            return True, "Installed"
        except ImportError:
            return False, "pip install markitdown"

    elif tool == "gemini":
        if not os.environ.get("GEMINI_API_KEY"):
            return False, "GEMINI_API_KEY not set"
        try:
            import google.generativeai
            return True, "Ready"
        except ImportError:
            return False, "pip install google-generativeai"

    elif tool == "claude":
        if not os.environ.get("ANTHROPIC_API_KEY"):
            return False, "ANTHROPIC_API_KEY not set"
        try:
            import anthropic
            return True, "Ready"
        except ImportError:
            return False, "pip install anthropic"

    return False, "Unknown tool"


def list_available_tools():
    """Print status of all tools."""
    print("Available extraction tools:")
    print("-" * 50)
    for tool, description in AVAILABLE_TOOLS.items():
        available, status = check_tool_available(tool)
        icon = "[OK]" if available else "[--]"
        print(f"  {icon} {tool}: {description}")
        if not available:
            print(f"       Setup: {status}")
    print()


if __name__ == "__main__":
    list_available_tools()
