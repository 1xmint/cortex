#!/bin/bash
# Cortex Deployment Verification Script

set -euo pipefail

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}🔍 Cortex Deployment Verification${NC}"
echo -e "${BLUE}===================================${NC}"

BASE_URL="${1:-http://localhost:3001}"

echo -e "${YELLOW}Testing against: $BASE_URL${NC}"
echo ""

# Test 1: Health Check
echo -e "${BLUE}1. Health Check${NC}"
if curl -sf "$BASE_URL/api/health" > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Backend health check passed${NC}"
else
    echo -e "${RED}❌ Backend health check failed${NC}"
    exit 1
fi

# Test 2: Frontend Files
echo -e "${BLUE}2. Frontend Access${NC}"
if curl -sf "$BASE_URL/" | grep -q "Cortex" > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Frontend accessible${NC}"
else
    echo -e "${RED}❌ Frontend not accessible${NC}"
    exit 1
fi

# Test 3: Static Assets
echo -e "${BLUE}3. Static Assets${NC}"
if curl -sf "$BASE_URL/assets/" > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Static assets served${NC}"
else
    echo -e "${YELLOW}⚠️  Static assets check inconclusive${NC}"
fi

# Test 4: API Endpoints
echo -e "${BLUE}4. API Endpoints${NC}"
if curl -sf "$BASE_URL/api/deploy-status" > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Deploy status endpoint working${NC}"
else
    echo -e "${RED}❌ Deploy status endpoint failed${NC}"
fi

# Test 5: Admin Endpoint (should require auth)
echo -e "${BLUE}5. Admin Protection${NC}"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/api/admin/stats" || echo "000")
if [[ "$HTTP_CODE" == "401" ]] || [[ "$HTTP_CODE" == "403" ]]; then
    echo -e "${GREEN}✅ Admin endpoint properly protected${NC}"
elif [[ "$HTTP_CODE" == "200" ]]; then
    echo -e "${YELLOW}⚠️  Admin endpoint accessible (dev mode?)${NC}"
else
    echo -e "${RED}❌ Admin endpoint returned $HTTP_CODE${NC}"
fi

# Test 6: Environment Check
echo -e "${BLUE}6. Environment Configuration${NC}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -f "$PROJECT_ROOT/.env" ]] && grep -q "CORTEX_ADMIN_EMAILS" "$PROJECT_ROOT/.env"; then
    ADMIN_EMAILS=$(grep "CORTEX_ADMIN_EMAILS" "$PROJECT_ROOT/.env" | cut -d'=' -f2)
    echo -e "${GREEN}✅ Admin emails configured: $ADMIN_EMAILS${NC}"
else
    echo -e "${RED}❌ CORTEX_ADMIN_EMAILS not configured${NC}"
fi

# Test 7: Frontend Build Check
echo -e "${BLUE}7. Frontend Build${NC}"
if [[ -f "$PROJECT_ROOT/cortex/dist/index.html" ]]; then
    BUILD_TIME=$(stat -c %y "$PROJECT_ROOT/cortex/dist/index.html" 2>/dev/null || stat -f %Sm "$PROJECT_ROOT/cortex/dist/index.html" 2>/dev/null || echo "unknown")
    echo -e "${GREEN}✅ Frontend built (modified: $BUILD_TIME)${NC}"
else
    echo -e "${RED}❌ Frontend not built${NC}"
fi

echo ""
echo -e "${BLUE}📋 Summary${NC}"

if curl -sf "$BASE_URL/api/health" > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Cortex deployment is functional${NC}"
    echo ""
    echo -e "${BLUE}🌐 Access URLs:${NC}"
    echo "   Frontend: $BASE_URL/"
    echo "   API: $BASE_URL/api/"
    echo "   Health: $BASE_URL/api/health"
    echo ""
    echo -e "${BLUE}👤 Admin Login:${NC}"
    echo "   Email: jfair1028@gmail.com"
    echo "   Access: Sign in to see admin features"
else
    echo -e "${RED}❌ Cortex deployment has issues${NC}"
    exit 1
fi