# Production CLI Authentication Setup

Status: canonical

This document provides step-by-step instructions for setting up Claude and Codex CLI authentication in production VPS environments.

## Overview

Cortex supports two authentication modes:
1. **BYOS (Bring Your Own Subscription)** - Users use their own Claude/OpenAI subscriptions
2. **BYOK (Bring Your Own Keys)** - Users provide API keys directly in the UI

This guide covers BYOS mode, which provides the best user experience and cost efficiency.

## Quick Setup

### 1. Get API Keys

**Claude/Anthropic:**
- Visit https://console.anthropic.com/account/keys
- Create a new API key
- Copy the `sk-ant-...` value

**OpenAI:**
- Visit https://platform.openai.com/account/api-keys
- Create a new API key
- Copy the `sk-proj-...` value

### 2. Set Environment Variables

```bash
# SSH into your VPS
ssh your-vps

# Create secure environment file
sudo mkdir -p /etc/cortex
sudo nano /etc/cortex/cli-auth.env

# Add your API keys:
ANTHROPIC_API_KEY=sk-ant-your-key-here
OPENAI_API_KEY=sk-proj-your-key-here

# Secure the file
sudo chmod 600 /etc/cortex/cli-auth.env
sudo chown root:root /etc/cortex/cli-auth.env
```

### 3. Update Service Configuration

```bash
# Edit the systemd service
sudo systemctl edit cortex

# Add these lines:
[Service]
EnvironmentFile=/etc/cortex/cli-auth.env

# Restart the service
sudo systemctl daemon-reload
sudo systemctl restart cortex
```

### 4. Verify Setup

```bash
# Run the verification script
sudo -u deploy bash /home/deploy/claw-net/scripts/verify-cli-auth.sh

# Check service logs
sudo journalctl -u cortex -f
```

## Docker Setup

### docker-compose.yml

```yaml
services:
  brain:
    environment:
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
      - OPENAI_API_KEY=${OPENAI_API_KEY}
```

### .env file

```bash
# Create .env file in project root
ANTHROPIC_API_KEY=sk-ant-your-key-here
OPENAI_API_KEY=sk-proj-your-key-here
```

## Automated Setup

Use the included scripts for automated setup:

```bash
# Set environment variables first
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-proj-..."

# Run automated setup
bash scripts/setup-headless-auth.sh

# Verify authentication
bash scripts/verify-cli-auth.sh
```

## Troubleshooting

### Common Issues

**"CLI not found"**
```bash
# Install Claude CLI
npm install -g @anthropic-ai/claude-cli

# Install Codex CLI  
npm install -g @openai/codex
```

**"Authentication failed"**
```bash
# Check if API keys are valid
curl -H "x-api-key: $ANTHROPIC_API_KEY" \
     -H "anthropic-version: 2023-06-01" \
     https://api.anthropic.com/v1/messages

curl -H "Authorization: Bearer $OPENAI_API_KEY" \
     https://api.openai.com/v1/models
```

**"Permission denied"**
```bash
# Check file ownership and permissions
ls -la ~/.anthropic/
ls -la ~/.codex/

# Fix ownership if needed
sudo chown -R deploy:deploy ~/.anthropic ~/.codex
```

### Manual Authentication

If automated setup fails, authenticate manually:

**Claude:**
```bash
# Switch to the service user
sudo -u deploy -s

# Run interactive login
claude auth login
```

**Codex:**
```bash
# Switch to the service user
sudo -u deploy -s

# Run device auth flow
codex login --device-auth
```

## Security Best Practices

1. **Store API keys securely**
   - Use `/etc/cortex/cli-auth.env` (root-owned, 600 permissions)
   - Never commit keys to version control
   - Use secrets management in production environments

2. **Rotate keys regularly**
   - Set up quarterly rotation reminders
   - Monitor usage through provider dashboards
   - Revoke old keys immediately after rotation

3. **Monitor usage**
   - Check Anthropic Console for Claude usage
   - Check OpenAI Platform for API usage
   - Set up billing alerts

4. **Principle of least privilege**
   - Create dedicated API keys for Cortex
   - Don't share keys between environments
   - Use separate keys for dev/staging/production

## Monitoring

### Health Checks

```bash
# Check authentication status
curl http://localhost:3001/api/auth/status

# Check service health
curl http://localhost:3001/api/health
```

### Log Monitoring

```bash
# Watch for auth failures
sudo journalctl -u cortex -f | grep -i "auth\|login\|key"

# Check CLI output
sudo journalctl -u cortex -f | grep -i "claude\|codex"
```

## Backup and Recovery

### Backup Auth Configuration

```bash
# Backup environment files
sudo cp /etc/cortex/cli-auth.env /backup/cli-auth-$(date +%Y%m%d).env

# Backup CLI auth files
sudo tar -czf /backup/cli-auth-files-$(date +%Y%m%d).tar.gz \
  /home/deploy/.anthropic /home/deploy/.codex
```

### Recovery Procedure

```bash
# Restore environment file
sudo cp /backup/cli-auth-YYYYMMDD.env /etc/cortex/cli-auth.env
sudo chmod 600 /etc/cortex/cli-auth.env

# Restart service
sudo systemctl restart cortex

# Re-run setup if needed
sudo -u deploy bash scripts/setup-headless-auth.sh
```

## Multiple Environment Setup

For organizations running multiple Cortex instances:

```bash
# Production
ANTHROPIC_API_KEY=sk-ant-prod-...
OPENAI_API_KEY=sk-proj-prod-...

# Staging
ANTHROPIC_API_KEY=sk-ant-staging-...
OPENAI_API_KEY=sk-proj-staging-...

# Development (optional)
ANTHROPIC_API_KEY=sk-ant-dev-...
OPENAI_API_KEY=sk-proj-dev-...
```

## Cost Management

1. **Set usage limits** in provider dashboards
2. **Monitor monthly spend** and set alerts
3. **Use cheaper models** for development/testing
4. **Implement rate limiting** at the application level
5. **Regular usage audits** to identify optimization opportunities