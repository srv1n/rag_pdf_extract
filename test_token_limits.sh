#!/bin/bash

# Test script to verify token limits are respected

echo "Building the extract example..."
cargo build --example extract 2>/dev/null

if [ $? -ne 0 ]; then
    echo "Build failed!"
    exit 1
fi

echo "Testing with 350 token limit..."
OUTPUT=$(./target/debug/examples/extract "/Users/sarav/Downloads/Dad case/6. Additonal Affidavit dated 03.07.2025.docx.pdf" 350 2>&1)

# Count warnings about exceeding token limits
WARNINGS=$(echo "$OUTPUT" | grep -c "exceeds max_tokens")
CRITICAL_WARNINGS=$(echo "$OUTPUT" | grep "exceeds max_tokens" | grep -E "[4-9][0-9]{2}|[0-9]{4,}" | wc -l)

echo "Total warnings about exceeding limits: $WARNINGS"
echo "Critical warnings (>400 tokens): $CRITICAL_WARNINGS"

# Check for the specific 406 token issue
if echo "$OUTPUT" | grep -q "406 actual tokens"; then
    echo "❌ FAILED: Still seeing 406 token chunks"
    echo "Sample:"
    echo "$OUTPUT" | grep -A 2 -B 2 "406 actual tokens" | head -20
else
    echo "✅ PASSED: No more 406 token chunks"
fi

# Check for any chunks over 400 tokens (allowing 50 token overshoot)
OVER_400=$(echo "$OUTPUT" | grep "actual tokens" | awk '{print $6}' | awk '$1 > 400' | wc -l)
if [ $OVER_400 -gt 0 ]; then
    echo "⚠️  WARNING: Found $OVER_400 chunks over 400 tokens (max allowed: 350 + 50 overshoot)"
    echo "Token counts over 400:"
    echo "$OUTPUT" | grep "actual tokens" | awk '{print $6}' | awk '$1 > 400' | sort -n | uniq
else
    echo "✅ PASSED: All chunks within acceptable range (≤400 tokens)"
fi

echo ""
echo "Summary:"
echo "- Max expected: 350 tokens"
echo "- Max with overshoot: 400 tokens (350 + 50 for sentence completion)"
echo "- Warnings found: $WARNINGS"
echo "- Critical issues: $CRITICAL_WARNINGS"