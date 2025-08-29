#!/bin/bash

# Test script to verify color space handling

echo "Testing PDF with various color spaces..."

# Build the extract example
cargo build --example extract 2>/dev/null

if [ $? -ne 0 ]; then
    echo "Build failed!"
    exit 1
fi

# Function to test a PDF file
test_pdf() {
    local pdf_path="$1"
    local pdf_name=$(basename "$pdf_path")
    
    echo ""
    echo "Testing: $pdf_name"
    echo "----------------------------------------"
    
    # Run extraction and capture errors
    OUTPUT=$(./target/debug/examples/extract "$pdf_path" 350 2>&1)
    
    # Count different types of color space errors
    INDEXED_ERRORS=$(echo "$OUTPUT" | grep -c "Unsupported color space.*Indexed")
    NONE_ERRORS=$(echo "$OUTPUT" | grep -c "Unsupported color space.*None")
    OTHER_ERRORS=$(echo "$OUTPUT" | grep -c "Unsupported color space" | awk -v idx=$INDEXED_ERRORS -v none=$NONE_ERRORS '{print $1 - idx - none}')
    
    # Count successful conversions
    INDEXED_SUCCESS=$(echo "$OUTPUT" | grep -c "Converting Indexed color space")
    RGB_SUCCESS=$(echo "$OUTPUT" | grep -c "Converting RGB image")
    GRAY_SUCCESS=$(echo "$OUTPUT" | grep -c "Converting grayscale image")
    CMYK_SUCCESS=$(echo "$OUTPUT" | grep -c "Converting CMYK image")
    
    echo "Errors:"
    echo "  - Indexed color space errors: $INDEXED_ERRORS"
    echo "  - None color space errors: $NONE_ERRORS"
    echo "  - Other color space errors: $OTHER_ERRORS"
    echo ""
    echo "Successful conversions:"
    echo "  - Indexed: $INDEXED_SUCCESS"
    echo "  - RGB: $RGB_SUCCESS"
    echo "  - Grayscale: $GRAY_SUCCESS"
    echo "  - CMYK: $CMYK_SUCCESS"
    
    if [ $INDEXED_ERRORS -gt 0 ] || [ $NONE_ERRORS -gt 0 ]; then
        echo ""
        echo "⚠️  Still seeing color space errors!"
        echo "Sample errors:"
        echo "$OUTPUT" | grep "Unsupported color space" | head -3
    else
        echo ""
        echo "✅ No unsupported color space errors"
    fi
}

# Test the problematic PDF
test_pdf "/Users/sarav/Downloads/Dad case/6. Additonal Affidavit dated 03.07.2025.docx.pdf"

# Test any other PDFs in the directory
for pdf in /Users/sarav/Downloads/Dad\ case/*.pdf; do
    if [ -f "$pdf" ] && [ "$pdf" != "/Users/sarav/Downloads/Dad case/6. Additonal Affidavit dated 03.07.2025.docx.pdf" ]; then
        test_pdf "$pdf"
        break  # Just test one more for now
    fi
done