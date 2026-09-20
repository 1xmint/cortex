# Cortex Production Deployment

Production-ready deployment configuration for the Cortex conversational AI orchestration system.

## 🚀 Quick Start

```bash
# Deploy to production
sudo ./deploy-production.sh

# Control the service
sudo ./cortex-control.sh start
sudo ./cortex-control.sh health
sudo ./cortex-control.sh status
```

## 📁 Files

- **`deploy-production.sh`** - Complete production deployment script
- **`cortex-control.sh`** - Service management interface
- **`health-check.sh`** - Comprehensive health monitoring
- **`cortex-api.service`** - Systemd service definition
- **`nginx-cortex.conf`** - Nginx reverse proxy configuration

## 🏗️ Architecture

```
Internet → Nginx (443) → Cortex API (3001) → SQLite/PostgreSQL
                     ↓
                Static Frontend Files
```

### Components

1. **Rust Backend** - Conversational orchestration API server
2. **React Frontend** - Built static files served by nginx
3. **Nginx** - Reverse proxy, SSL termination, static file serving
4. **Systemd** - Service management with auto-restart
5. **Monitoring** - Health checks every 2 minutes with auto-recovery

## 🛠️ Installation

### Prerequisites

- Linux server (Ubuntu 20.04+ or similar)
- Rust toolchain (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Node.js 18+ (`curl -fsSL https://deb.nodesource.com/setup_18.x | sudo -E bash - && sudo apt-get install -y nodejs`)
- Nginx (`sudo apt install nginx`)
- Optional: PostgreSQL for production database

### Deploy

```bash
# Clone the repository
git clone <your-repo-url>
cd cortex

# Run deployment script
sudo ./deploy/deploy-production.sh
```

This will:
- Create `cortex` system user
- Build and install the Rust backend
- Build and install the React frontend
- Configure systemd service
- Set up nginx reverse proxy
- Start services with auto-restart
- Configure health monitoring

## 🔧 Configuration

### Environment Variables

Edit `/opt/cortex/.env`:

```bash
# Core settings
CORTEX_PORT=3001
CORTEX_WORKSPACE=/opt/cortex/workspace
CORTEX_LEDGER_PATH=/opt/cortex/data/ledger.jsonl
CORTEX_ALLOWED_ORIGINS=https://cortex.heyvera.org

# Database (optional - defaults to SQLite)
CORTEX_DATABASE_URL=postgresql://user:pass@localhost/cortex

# Authentication
CLERK_SECRET_KEY=your_clerk_secret_key

# GitHub Integration
GITHUB_TOKEN=your_github_token

# Billing (optional)
STRIPE_SECRET_KEY=your_stripe_secret_key
STRIPE_WEBHOOK_SECRET=your_webhook_secret
```

### SSL Certificates

Install SSL certificates using Let's Encrypt:

```bash
sudo apt install certbot python3-certbot-nginx
sudo certbot --nginx -d cortex.heyvera.org
```

## 🎛️ Service Management

### Using cortex-control.sh

```bash
# Start/stop/restart
sudo ./deploy/cortex-control.sh start
sudo ./deploy/cortex-control.sh stop
sudo ./deploy/cortex-control.sh restart

# Status and health
sudo ./deploy/cortex-control.sh status
sudo ./deploy/cortex-control.sh health

# Logs
sudo ./deploy/cortex-control.sh logs

# Backup/restore
sudo ./deploy/cortex-control.sh backup
sudo ./deploy/cortex-control.sh restore
```

### Manual Service Commands

```bash
# Service management
sudo systemctl start cortex-api
sudo systemctl stop cortex-api
sudo systemctl restart cortex-api
sudo systemctl status cortex-api

# Enable auto-start on boot
sudo systemctl enable cortex-api

# View logs
sudo journalctl -fu cortex-api
```

## 📊 Monitoring

### Automatic Health Monitoring

The system includes automatic health monitoring that:
- Runs every 2 minutes via cron
- Checks service status and API health
- Automatically restarts on failure
- Logs all activities

Monitor logs:
```bash
sudo tail -f /var/log/cortex-monitor.log
```

### Manual Health Checks

```bash
# Comprehensive health check
./deploy/health-check.sh

# Basic API health
curl http://localhost:3001/api/health

# Test conversational flows
curl -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer test-token" \
  -d '{"message": "I want to build a habit tracker"}' \
  http://localhost:3001/api/flow/test
```

### Metrics and Logs

- **Service logs**: `journalctl -fu cortex-api`
- **Access logs**: `/var/log/nginx/cortex.access.log`
- **Error logs**: `/var/log/nginx/cortex.error.log`
- **Monitor logs**: `/var/log/cortex-monitor.log`

## 🔄 Updates and Deployment

### Deploy Updates

```bash
# Pull latest code
git pull

# Deploy new version
sudo ./deploy/deploy-production.sh

# Verify deployment
sudo ./deploy/cortex-control.sh health
```

### Zero-Downtime Deployment

For zero-downtime deployments:

1. Build new version
2. Health check staging
3. Deploy behind load balancer
4. Switch traffic
5. Remove old instance

## 🔒 Security

### Firewall Configuration

```bash
# Allow SSH, HTTP, and HTTPS
sudo ufw allow 22/tcp
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp

# Block direct API access (nginx handles proxy)
sudo ufw deny 3001/tcp

# Enable firewall
sudo ufw enable
```

### SSL Configuration

The nginx configuration includes:
- HTTPS redirect
- Security headers (HSTS, XSS protection, etc.)
- Rate limiting
- CORS configuration

### File Permissions

- Service runs as dedicated `cortex` user
- Data directories have restricted access
- Environment file is mode 600 (owner read-only)
- Binaries are executable by service user only

## 🐛 Troubleshooting

### Common Issues

**Service won't start:**
```bash
# Check service status
sudo systemctl status cortex-api

# Check logs
sudo journalctl -fu cortex-api

# Verify binary
ls -la /opt/cortex/bin/cortex-server

# Test manually
sudo -u cortex /opt/cortex/bin/cortex-server
```

**API not responding:**
```bash
# Check if port is listening
sudo ss -tlnp | grep :3001

# Test local connection
curl -v http://localhost:3001/api/health

# Check nginx proxy
sudo nginx -t
sudo systemctl status nginx
```

**High memory usage:**
```bash
# Check process memory
ps aux | grep cortex-server

# Check system memory
free -h

# Restart service if needed
sudo systemctl restart cortex-api
```

**Conversational flows not working:**
```bash
# Test flow endpoint
curl -X POST \
  -H "Content-Type: application/json" \
  -d '{"message": "build a habit tracker"}' \
  http://localhost:3001/api/flow/test

# Check intent classification
grep "Setup" /opt/cortex/data/ledger.jsonl
```

### Emergency Recovery

If the service is completely broken:

```bash
# Stop everything
sudo systemctl stop cortex-api nginx

# Restore from backup
sudo ./deploy/cortex-control.sh restore

# Restart services
sudo systemctl start nginx cortex-api

# Verify health
sudo ./deploy/cortex-control.sh health
```

## 📚 Documentation

- [Cortex API Documentation](../crates/api/README.md)
- [Frontend Documentation](../cortex/README.md)
- [Configuration Reference](../docs/configuration.md)

## 🆘 Support

If you encounter issues:

1. Check the troubleshooting section above
2. Run the health check: `./deploy/health-check.sh -v`
3. Collect logs: `sudo journalctl -fu cortex-api --since "1 hour ago"`
4. Review configuration: `sudo cat /opt/cortex/.env`