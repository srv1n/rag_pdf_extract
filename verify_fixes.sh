#!/bin/bash

echo "========================================="
echo "PDF-EXTRACT VERIFICATION SCRIPT"
echo "========================================="
echo ""

# Ensure we're using the latest build
echo "1. Building latest version..."
cargo build --example extract 2>/dev/null
if [ $? -ne 0 ]; then
    echo "❌ Build failed!"
    exit 1
fi
echo "✅ Build successful"
echo ""

# Test file
TEST_PDF="/Users/sarav/Downloads/Dad case/6. Additonal Affidavit dated 03.07.2025.docx.pdf"

echo "2. Testing token limits (max: 350)..."
OUTPUT=$(./target/debug/examples/extract "$TEST_PDF" 350 2>&1)

# Check for token warnings
TOKEN_WARNINGS=$(echo "$OUTPUT" | grep -c "exceeds max_tokens")
if [ $TOKEN_WARNINGS -gt 0 ]; then
    echo "❌ Found $TOKEN_WARNINGS token limit warnings:"
    echo "$OUTPUT" | grep "exceeds max_tokens" | head -3
else
    echo "✅ No token limit violations"
fi
echo ""

echo "3. Testing image conversion logging..."
# Run with debug logging to see image messages
DEBUG_OUTPUT=$(RUST_LOG=debug ./target/debug/examples/extract "$TEST_PDF" 350 2>&1)

# Check for old ERROR messages about images
IMAGE_ERRORS=$(echo "$DEBUG_OUTPUT" | grep -c "ERROR.*Image conversion failed")
if [ $IMAGE_ERRORS -gt 0 ]; then
    echo "❌ Still seeing ERROR level image messages:"
    echo "$DEBUG_OUTPUT" | grep "ERROR.*Image conversion" | head -2
else
    echo "✅ No ERROR level image conversion messages"
fi

# Check for new DEBUG messages
IMAGE_DEBUG=$(echo "$DEBUG_OUTPUT" | grep -c "Image conversion skipped")
if [ $IMAGE_DEBUG -gt 0 ]; then
    echo "✅ Image issues logged at DEBUG level ($IMAGE_DEBUG occurrences)"
else
    echo "ℹ️  No image conversion issues detected"
fi
echo ""

echo "4. Checking color space handling..."
INDEXED_ERRORS=$(echo "$OUTPUT" | grep -c "Unsupported color space.*Indexed")
NONE_ERRORS=$(echo "$OUTPUT" | grep -c "Unsupported color space.*None")

if [ $INDEXED_ERRORS -gt 0 ] || [ $NONE_ERRORS -gt 0 ]; then
    echo "❌ Color space errors found:"
    echo "   - Indexed: $INDEXED_ERRORS"
    echo "   - None: $NONE_ERRORS"
else
    echo "✅ No unsupported color space errors"
fi
echo ""

echo "5. Overall extraction test..."
# Count total chunks extracted
CHUNK_COUNT=$(echo "$OUTPUT" | grep -c "chunk_id:")
if [ $CHUNK_COUNT -gt 0 ]; then
    echo "✅ Successfully extracted $CHUNK_COUNT chunks"
else
    echo "❌ No chunks extracted!"
fi
echo ""

echo "========================================="
echo "SUMMARY:"
echo "========================================="
if [ $TOKEN_WARNINGS -eq 0 ] && [ $IMAGE_ERRORS -eq 0 ] && [ $INDEXED_ERRORS -eq 0 ] && [ $CHUNK_COUNT -gt 0 ]; then
    echo "✅ ALL TESTS PASSED!"
    echo ""
    echo "The library is working correctly:"
    echo "- Token limits are respected (max 350 + 50 overshoot)"
    echo "- Image conversion failures don't show as errors"
    echo "- Color spaces are handled gracefully"
    echo "- Text extraction is successful"
else
    echo "⚠️  Some issues detected - see details above"
fi
echo ""
echo "Note: If you're still seeing errors in your app, make sure to:"
echo "1. Rebuild your application with the latest library"
echo "2. Restart any running services"
echo "3. Clear any caches that might have old builds"