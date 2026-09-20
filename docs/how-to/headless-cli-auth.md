# Headless CLI Authentication Guide

Status: implementation

## Overview

This guide describes how to authenticate Claude and Codex CLI tools in headless environments (VPS containers, CI/CD, etc.) where interactive OAuth flows aren't possible.

## Authentication Methods

### Option 1: Environment Variables (Recommended)

Set these environment variables before starting the Cortex service:

```bash
# Claude Authentication
export ANTHROPIC_API_KEY="sk-ant-..."

# OpenAI Authentication  
export OPENAI_API_KEY="sk-proj-..."
```

The system will automatically detect these and configure CLI authentication. If CLI authentication fails, the system will automatically fall back to direct API mode using the same keys.

**Note:** This provides the best reliability as it works even if CLI tools aren't available or authenticated.

### Option 2: Device Code Flow

For environments with limited browser access but some connectivity:

```bash
# Start device auth flow
codex login --device-auth
# Follow the displayed URL and enter the device code
```

### Option 3: Manual Token Configuration

If you have existing tokens:

```bash
# Claude (requires manual file placement)
mkdir -p ~/.anthropic
echo '{"access_token": "your_token"}' > ~/.anthropic/auth.json

# Codex via stdin
echo "your_api_key" | codex login --with-api-key
```

### Option 4: Admin UI Flow

Use the Cortex admin interface at `/settings` to authenticate providers:

1. Click "Connect Provider"
2. Follow the device auth flow
3. Enter the code when prompted

## Container Setup

### Docker Environment Variables

Add to your `.env` file or docker-compose.yml:

```bash
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-proj-...
```

### Kubernetes Secrets

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: cortex-cli-auth
type: Opaque
stringData:
  anthropic_api_key: "sk-ant-..."
  openai_api_key: "sk-proj-..."
```

## VPS Production Setup

### 1. Create Secure Environment File

```bash
sudo mkdir -p /etc/cortex
sudo nano /etc/cortex/cli-auth.env

# Add:
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-proj-...

sudo chmod 600 /etc/cortex/cli-auth.env
sudo chown root:root /etc/cortex/cli-auth.env
```

### 2. Update Service Configuration

```bash
# Add to /etc/systemd/system/cortex.service
EnvironmentFile=/etc/cortex/cli-auth.env

sudo systemctl daemon-reload
sudo systemctl restart cortex
```

## Troubleshooting

### Check Authentication Status

```bash
# Check Claude
claude auth status --json

# Check Codex  
codex login status

# Check via API
curl http://localhost:3001/api/auth/status
```

### Common Issues

**"Binary not found"**: Install CLI tools
```bash
npm install -g @anthropic-ai/claude-cli
npm install -g @openai/codex
```

**"Authentication failed"**: Verify API keys
```bash
# Test Claude key
curl https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model": "claude-haiku-4-5", "max_tokens": 10, "messages": [{"role": "user", "content": "Hi"}]}'

# Test OpenAI key  
curl https://api.openai.com/v1/models \
  -H "Authorization: Bearer $OPENAI_API_KEY"
```

**"Permission denied"**: Check file ownership
```bash
ls -la ~/.anthropic/
ls -la ~/.codex/
```

## Security Best Practices

1. **Never commit API keys** to version control
2. **Use secrets management** in production (HashiCorp Vault, AWS Secrets Manager, etc.)
3. **Rotate keys regularly** (quarterly for API keys)
4. **Monitor usage** through provider dashboards
5. **Use principle of least privilege** - separate keys for different environments

## Automation Scripts

The system includes automated setup scripts:

- `/scripts/setup-headless-auth.sh` - Automated environment detection and setup
- `/scripts/verify-cli-auth.sh` - Validation script for CI/CD pipelines