#!/bin/bash
set -euo pipefail

# Build Cortex Frontend for Production
# This script builds the Cortex React frontend and prepares it for production deployment

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${GREEN}🔨 Building Cortex frontend for production...${NC}"

# Navigate to cortex directory
cd "$(dirname "$0")/../cortex"

# Check if we're in the right directory
if [[ ! -f package.json ]]; then
    echo -e "${RED}❌ Error: package.json not found in cortex directory${NC}"
    exit 1
fi

# Install dependencies
echo -e "${YELLOW}📦 Installing dependencies...${NC}"
npm ci

# Build the frontend
echo -e "${YELLOW}⚛️ Building React app...${NC}"
npm run build

# Verify build succeeded
if [[ ! -d dist ]] || [[ ! -f dist/index.html ]]; then
    echo -e "${RED}❌ Build failed - dist/index.html not found${NC}"
    exit 1
fi

echo -e "${GREEN}✅ Frontend built successfully${NC}"

# Production deployment strategy
if [[ "${1:-}" == "--prod" ]] || [[ "${CORTEX_ENV:-}" == "production" ]]; then
    echo -e "${BLUE}🚀 Production deployment mode${NC}"

    # Option 1: Embedded in Rust binary (current approach)
    echo -e "${YELLOW}📁 Built files ready for Rust backend serving${NC}"
    echo -e "${YELLOW}📍 Location: $(pwd)/dist${NC}"
    echo -e "${YELLOW}🎯 Backend will serve from CORTEX_STATIC_DIR=cortex/dist${NC}"

    # Show file sizes
    echo -e "${BLUE}📊 Build output:${NC}"
    ls -lah dist/

    echo ""
    echo -e "${GREEN}✅ Production build complete!${NC}"
    echo -e "${YELLOW}📋 Next steps for VPS deployment:${NC}"
    echo "   1. Ensure CORTEX_ADMIN_EMAILS=jfair1028@gmail.com in environment"
    echo "   2. Set CORTEX_STATIC_DIR=cortex/dist (default)"
    echo "   3. Rebuild and restart the Rust backend"
    echo "   4. Frontend will be served at cortex.heyvera.org/"

else
    # Development mode
    echo -e "${BLUE}🔧 Development mode${NC}"
    echo -e "${GREEN}✅ Build complete!${NC}"
    echo -e "${YELLOW}📋 For local testing:${NC}"
    echo "   1. Set CORTEX_STATIC_DIR=cortex/dist"
    echo "   2. Run: cargo run --bin cortex-server"
    echo "   3. Access: http://localhost:3001"
fi