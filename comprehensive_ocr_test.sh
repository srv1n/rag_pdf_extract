#!/bin/bash

echo "=== Comprehensive OCR Test ==="
echo "=============================="

# Clean up
rm -rf debug_images test_original test_scaled
mkdir -p test_original test_scaled

echo -e "\n1. Extracting with NO downscaling..."
SKIP_DOWNSCALE=1 cargo run --release --example extract -- ocr.pdf 1 --ocr models/text-detection-ssfbcj81.rten models/text-rec-checkpoint-s52qdbqt.rten 2>&1 | grep -E "📝 OCR result preview" | head -5

# Move original size images
mv debug_images/* test_original/ 2>/dev/null

echo -e "\n2. Extracting WITH downscaling..."
cargo run --release --example extract -- ocr.pdf 1 --ocr models/text-detection-ssfbcj81.rten models/text-rec-checkpoint-s52qdbqt.rten 2>&1 | grep -E "📝 OCR result preview" | head -5

# Move scaled images
mv debug_images/* test_scaled/ 2>/dev/null

echo -e "\n3. Testing ORIGINAL size image with OCRS CLI..."
ORIGINAL=$(ls test_original/*.png | head -1)
echo "Image: $ORIGINAL"
file "$ORIGINAL" | grep -o "[0-9]* x [0-9]*"
ocrs "$ORIGINAL" --detect-model models/text-detection-ssfbcj81.rten --rec-model models/text-rec-checkpoint-s52qdbqt.rten 2>/dev/null | head -10

echo -e "\n4. Testing SCALED image with OCRS CLI..."
SCALED=$(ls test_scaled/*.png | head -1)
echo "Image: $SCALED"
file "$SCALED" | grep -o "[0-9]* x [0-9]*"
ocrs "$SCALED" --detect-model models/text-detection-ssfbcj81.rten --rec-model models/text-rec-checkpoint-s52qdbqt.rten 2>/dev/null | head -10

echo -e "\n=== Summary ==="
echo "Original size images in: test_original/"
echo "Scaled images in: test_scaled/"
echo "Compare the OCR quality between original and scaled images."