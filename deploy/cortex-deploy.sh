#!/bin/bash
# Cortex Production Deployment Script
# This script handles the complete deployment pipeline for Cortex

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Default values
SKIP_BUILD=false
SKIP_BACKEND=false
DRY_RUN=false

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --skip-backend)
            SKIP_BACKEND=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  --skip-build    Skip frontend build step"
            echo "  --skip-backend  Skip backend rebuild step"
            echo "  --dry-run       Show what would be done without executing"
            echo "  -h, --help      Show this help message"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

echo -e "${CYAN}🚀 Cortex Production Deployment${NC}"
echo -e "${CYAN}================================${NC}"

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# Verify we're in the right place
if [[ ! -f "Cargo.toml" ]] || [[ ! -d "cortex" ]]; then
    echo -e "${RED}❌ Error: Must run from project root with Cargo.toml and cortex/ directory${NC}"
    exit 1
fi

# Function to run commands with dry-run support
run_cmd() {
    if [[ "$DRY_RUN" == "true" ]]; then
        echo -e "${YELLOW}[DRY RUN] Would run: $*${NC}"
    else
        "$@"
    fi
}

# Step 1: Build Frontend
if [[ "$SKIP_BUILD" == "false" ]]; then
    echo -e "${BLUE}📦 Step 1: Building Cortex Frontend${NC}"

    cd cortex

    # Check Node.js version
    if ! command -v node &> /dev/null; then
        echo -e "${RED}❌ Node.js not found. Please install Node.js 18+${NC}"
        exit 1
    fi

    NODE_VERSION=$(node -v | sed 's/v//')
    MAJOR_VERSION=$(echo "$NODE_VERSION" | cut -d. -f1)
    if [[ "$MAJOR_VERSION" -lt 18 ]]; then
        echo -e "${YELLOW}⚠️  Warning: Node.js version $NODE_VERSION detected. Recommended: 18+${NC}"
    fi

    echo -e "${YELLOW}📦 Installing dependencies...${NC}"
    run_cmd npm ci

    echo -e "${YELLOW}⚛️  Building React frontend...${NC}"
    run_cmd npm run build

    if [[ "$DRY_RUN" == "false" ]] && [[ ! -f "dist/index.html" ]]; then
        echo -e "${RED}❌ Frontend build failed - index.html not found${NC}"
        exit 1
    fi

    echo -e "${GREEN}✅ Frontend build complete${NC}"
    cd "$PROJECT_ROOT"
else
    echo -e "${YELLOW}⏭️  Skipping frontend build${NC}"
fi

# Step 2: Environment Configuration
echo -e "${BLUE}🔧 Step 2: Environment Configuration${NC}"

# Check for admin email configuration
if [[ -f ".env" ]]; then
    if grep -q "CORTEX_ADMIN_EMAILS" .env; then
        ADMIN_EMAILS=$(grep "CORTEX_ADMIN_EMAILS" .env | cut -d'=' -f2)
        echo -e "${GREEN}✅ Admin emails configured: $ADMIN_EMAILS${NC}"
    else
        echo -e "${YELLOW}⚠️  CORTEX_ADMIN_EMAILS not found in .env${NC}"
        echo -e "${CYAN}📝 Add this line to enable admin access:${NC}"
        echo "CORTEX_ADMIN_EMAILS=jfair1028@gmail.com"
    fi
else
    echo -e "${YELLOW}⚠️  No .env file found${NC}"
    echo -e "${CYAN}📝 Create .env with:${NC}"
    echo "CORTEX_ADMIN_EMAILS=jfair1028@gmail.com"
    echo "CORTEX_STATIC_DIR=cortex/dist"
    echo "CORTEX_ALLOWED_ORIGINS=https://cortex.heyvera.org"
fi

# Step 3: Backend Build (if not skipped)
if [[ "$SKIP_BACKEND" == "false" ]]; then
    echo -e "${BLUE}🦀 Step 3: Building Rust Backend${NC}"

    # Check Rust installation
    if ! command -v cargo &> /dev/null; then
        echo -e "${RED}❌ Rust/Cargo not found. Please install Rust${NC}"
        exit 1
    fi

    echo -e "${YELLOW}🔨 Building backend...${NC}"
    run_cmd cargo build --release --bin cortex-server

    if [[ "$DRY_RUN" == "false" ]] && [[ ! -f "target/release/cortex-server" ]]; then
        echo -e "${RED}❌ Backend build failed${NC}"
        exit 1
    fi

    echo -e "${GREEN}✅ Backend build complete${NC}"
else
    echo -e "${YELLOW}⏭️  Skipping backend build${NC}"
fi

# Step 4: Docker Deployment
echo -e "${BLUE}🐳 Step 4: Docker Deployment${NC}"

if command -v docker-compose &> /dev/null || command -v docker compose &> /dev/null; then
    echo -e "${YELLOW}🔄 Deploying with Docker Compose...${NC}"

    # Determine compose command
    if command -v docker-compose &> /dev/null; then
        COMPOSE_CMD="docker-compose"
    else
        COMPOSE_CMD="docker compose"
    fi

    # Show current status
    echo -e "${CYAN}📊 Current deployment status:${NC}"
    run_cmd $COMPOSE_CMD ps

    # Deploy
    echo -e "${YELLOW}🚀 Rebuilding and starting services...${NC}"
    run_cmd $COMPOSE_CMD down
    run_cmd $COMPOSE_CMD up -d --build

    echo -e "${YELLOW}⏳ Waiting for services to be healthy...${NC}"
    if [[ "$DRY_RUN" == "false" ]]; then
        sleep 10
        run_cmd $COMPOSE_CMD ps
    fi

    echo -e "${GREEN}✅ Docker deployment complete${NC}"
else
    echo -e "${YELLOW}⚠️  Docker Compose not found${NC}"
    echo -e "${CYAN}📝 For manual deployment:${NC}"
    echo "   1. Copy target/release/cortex-server to your server"
    echo "   2. Set environment variables"
    echo "   3. Start: ./cortex-server"
fi

# Step 5: Verification
echo -e "${BLUE}🔍 Step 5: Deployment Verification${NC}"

if [[ "$DRY_RUN" == "false" ]]; then
    echo -e "${YELLOW}🏥 Checking health endpoints...${NC}"

    # Wait a bit for startup
    sleep 5

    # Check health endpoint
    if curl -sf http://localhost:3001/api/health > /dev/null 2>&1; then
        echo -e "${GREEN}✅ Backend health check passed${NC}"
    else
        echo -e "${YELLOW}⚠️  Backend health check failed (may still be starting)${NC}"
    fi

    # Check admin endpoint
    if curl -sf http://localhost:3001/api/admin/stats > /dev/null 2>&1; then
        echo -e "${GREEN}✅ Admin endpoint responding${NC}"
    else
        echo -e "${YELLOW}⚠️  Admin endpoint not responding (auth required)${NC}"
    fi

    # Check static files
    if [[ -f "cortex/dist/index.html" ]]; then
        echo -e "${GREEN}✅ Frontend files available${NC}"
    else
        echo -e "${RED}❌ Frontend files missing${NC}"
    fi
fi

# Summary
echo -e "${CYAN}📋 Deployment Summary${NC}"
echo -e "${CYAN}=====================${NC}"

if [[ "$DRY_RUN" == "false" ]]; then
    echo -e "${GREEN}✅ Deployment completed successfully!${NC}"
    echo ""
    echo -e "${BLUE}🌐 Access Points:${NC}"
    echo "   • Frontend: https://cortex.heyvera.org/"
    echo "   • API: https://cortex.heyvera.org/api/"
    echo "   • Health: https://cortex.heyvera.org/api/health"
    echo ""
    echo -e "${BLUE}👤 Admin Access:${NC}"
    echo "   • Email: jfair1028@gmail.com"
    echo "   • Features: BYOK settings, system stats, user management"
    echo ""
    echo -e "${BLUE}🔧 Next Steps:${NC}"
    echo "   1. Test admin login at cortex.heyvera.org"
    echo "   2. Verify admin menu is visible"
    echo "   3. Test BYOK API key management"
    echo "   4. Monitor logs for any issues"
else
    echo -e "${YELLOW}🔍 Dry run completed - no changes made${NC}"
    echo -e "${BLUE}To execute: $0 (remove --dry-run)${NC}"
fi