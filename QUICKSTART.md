# Win-Top Quick Start Guide

## 🚀 Get Started in 3 Minutes

### Step 1: Install (1 minute)

```bash
# Clone the repository
git clone https://github.com/nickshui/Win-Top.git
cd Win-Top

# Install dependencies
pip install -r requirements.txt
```

### Step 2: Run (30 seconds)

#### Option A: Try the Demo (No GUI)
```bash
python demo.py
```

#### Option B: Launch Full Application
```bash
python src/main.py
```

### Step 3: Explore (1.5 minutes)

1. **System Monitor Tab** - See your CPU, memory, disk, and network usage
2. **Process Manager Tab** - View and manage running processes
3. **Network Manager Tab** - Check network connections
4. **Windows Commands Tab** - Execute built-in commands
5. **AI Assistant Tab** - Get intelligent help (requires API key)

## 🤖 Enable AI Features (Optional)

### Quick Setup

1. **Get an API Key** (choose one):
   - OpenAI: https://platform.openai.com/api-keys
   - Anthropic: https://console.anthropic.com/

2. **Configure Win-Top**:
   ```bash
   # Copy the example file
   cp .env.example .env
   
   # Edit .env and add your key
   nano .env  # or use any text editor
   ```

3. **Add your key**:
   ```env
   OPENAI_API_KEY=sk-your-key-here
   ```

4. **Restart Win-Top**

## 💡 Quick Tips

### Monitor Your System
- CPU and memory update every 2 seconds
- Green bars = good, yellow/orange = attention needed, red = critical

### Manage Processes
- Click "Refresh" to update the process list
- Select a process and click "Kill" to terminate it
- Sort by clicking column headers

### Use Network Manager
- Click "Refresh Connections" to update
- Look for unfamiliar processes using the network

### Execute Commands
- Select a command from the dropdown
- Add parameters if needed
- Click "Execute" to run

### Ask AI Questions
Examples:
- "Why is my computer slow?"
- "What is chrome.exe doing?"
- "How do I free up disk space?"
- "Is this CPU usage normal?"

## 📚 Learn More

- **Full Documentation**: [README.md](README.md)
- **Usage Guide**: [USAGE_GUIDE.md](USAGE_GUIDE.md)
- **API Documentation**: [API_DOCUMENTATION.md](API_DOCUMENTATION.md)
- **Feature Showcase**: [FEATURES.md](FEATURES.md)

## ❓ Common Issues

### "No module named 'psutil'"
```bash
pip install psutil
```

### "No module named 'PyQt6'"
```bash
pip install PyQt6
```

### AI not working
1. Check your .env file exists
2. Verify your API key is correct
3. Ensure you have internet connection

### Permission errors
- Run as Administrator on Windows
- Use `sudo` on Linux/macOS

## 🆘 Get Help

- **Issues**: https://github.com/nickshui/Win-Top/issues
- **Discussions**: https://github.com/nickshui/Win-Top/discussions
- **Ask AI**: Use the AI Assistant tab in the app!

---

Enjoy using Win-Top! 🎉
