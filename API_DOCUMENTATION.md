# Win-Top API Documentation

## Core Modules

### SystemMonitor

Monitor system resources including CPU, memory, disk, and network.

```python
from src.modules import SystemMonitor

monitor = SystemMonitor()

# Get CPU information
cpu_info = monitor.get_cpu_info()
# Returns: dict with total_usage, per_cpu, count, physical_count, frequency

# Get memory information
memory_info = monitor.get_memory_info()
# Returns: dict with virtual and swap memory details

# Get disk information
disk_info = monitor.get_disk_info()
# Returns: list of dicts with device, total, used, free, percent

# Get network information
network_info = monitor.get_network_info()
# Returns: dict with io_counters and interfaces
```

### ProcessManager

Manage and monitor system processes.

```python
from src.modules import ProcessManager

proc_mgr = ProcessManager()

# Get all processes
processes = proc_mgr.get_all_processes()
# Returns: list of dicts with pid, name, cpu_percent, memory_mb, etc.

# Get specific process by PID
process = proc_mgr.get_process_by_pid(1234)
# Returns: dict with detailed process information

# Kill a process
success = proc_mgr.kill_process(1234)
# Returns: bool indicating success

# Terminate a process gracefully
success = proc_mgr.terminate_process(1234)
# Returns: bool indicating success

# Get process tree
tree = proc_mgr.get_process_tree(1234)
# Returns: dict with parent and children processes
```

### NetworkManager

Manage network connections and ports.

```python
from src.modules import NetworkManager

net_mgr = NetworkManager()

# Get all network connections
connections = net_mgr.get_connections(kind='inet')
# Returns: list of dicts with connection details

# Get listening ports
listening = net_mgr.get_listening_ports()
# Returns: list of dicts with port, address, pid, process_name

# Get connections by PID
conn_list = net_mgr.get_connections_by_pid(1234)
# Returns: list of dicts with connection details for the process

# Get network statistics
stats = net_mgr.get_network_stats()
# Returns: dict with interface statistics
```

### AIAssistant

AI-powered system management assistant.

```python
from src.modules import AIAssistant

ai = AIAssistant(provider="openai")  # or "anthropic"

# Analyze system status
system_data = {
    'cpu': monitor.get_cpu_info(),
    'memory': monitor.get_memory_info(),
    'disk': monitor.get_disk_info(),
    'network': monitor.get_network_info()
}
analysis = ai.analyze_system_status(system_data)
# Returns: str with AI analysis

# Diagnose a process
process_data = proc_mgr.get_process_by_pid(1234)
diagnosis = ai.diagnose_process(process_data)
# Returns: str with AI diagnosis

# Explain a Windows command
explanation = ai.explain_windows_command("netstat -ano")
# Returns: str with command explanation

# Get optimization suggestions
suggestions = ai.suggest_optimization("High CPU usage")
# Returns: str with optimization suggestions

# Answer questions
answer = ai.answer_question("Why is my computer slow?", system_context)
# Returns: str with AI answer
```

### WindowsCommands

Execute built-in Windows commands.

```python
from src.modules.windows_commands import WindowsCommands

cmd = WindowsCommands()

# Get available commands
commands = cmd.get_available_commands()
# Returns: dict mapping command names to descriptions

# Execute ipconfig
success, output = cmd.ipconfig("/all")
# Returns: tuple (bool, str)

# Execute netstat
success, output = cmd.netstat("-ano")
# Returns: tuple (bool, str)

# Execute ping
success, output = cmd.ping("8.8.8.8", count=4)
# Returns: tuple (bool, str)

# Kill process by PID
success, output = cmd.taskkill(pid=1234, force=True)
# Returns: tuple (bool, str)

# Flush DNS cache
success, output = cmd.flush_dns()
# Returns: tuple (bool, str)
```

## Configuration

### Config

Application configuration manager.

```python
from src.utils import Config

# Check if AI is configured
is_configured = Config.is_ai_configured()

# Get all configuration
config = Config.get_config()

# Access individual settings
provider = Config.AI_PROVIDER
api_key = Config.OPENAI_API_KEY
refresh = Config.REFRESH_INTERVAL
```

## UI Components

### MainWindow

Main application window with PyQt6.

```python
from PyQt6.QtWidgets import QApplication
from src.ui import MainWindow

app = QApplication(sys.argv)
window = MainWindow()
window.show()
app.exec()
```

## Examples

### Example 1: Monitor CPU for 10 seconds

```python
import time
from src.modules import SystemMonitor

monitor = SystemMonitor()

for i in range(10):
    cpu = monitor.get_cpu_info()
    print(f"CPU Usage: {cpu['total_usage']:.1f}%")
    time.sleep(1)
```

### Example 2: Find processes using most memory

```python
from src.modules import ProcessManager

proc_mgr = ProcessManager()
processes = proc_mgr.get_all_processes()

# Sort by memory usage
sorted_procs = sorted(processes, key=lambda x: x.get('memory_mb', 0), reverse=True)

# Show top 5
for proc in sorted_procs[:5]:
    print(f"{proc['name']}: {proc['memory_mb']:.1f} MB")
```

### Example 3: Monitor network speed

```python
import time
from src.modules import SystemMonitor

monitor = SystemMonitor()

while True:
    network = monitor.get_network_info()
    io = network['io_counters']
    print(f"↑ {io['upload_speed_mbps']:.2f} MB/s  ↓ {io['download_speed_mbps']:.2f} MB/s")
    time.sleep(2)
```

### Example 4: AI system analysis

```python
from src.modules import SystemMonitor, AIAssistant

monitor = SystemMonitor()
ai = AIAssistant()

system_data = {
    'cpu': monitor.get_cpu_info(),
    'memory': monitor.get_memory_info(),
    'disk': monitor.get_disk_info(),
    'network': monitor.get_network_info()
}

analysis = ai.analyze_system_status(system_data)
print(analysis)
```

### Example 5: Find which process is using a port

```python
from src.modules import NetworkManager

net_mgr = NetworkManager()
connections = net_mgr.get_connections()

target_port = 8080
for conn in connections:
    if f":{target_port}" in conn['local_address']:
        print(f"Port {target_port} is used by: {conn['process_name']} (PID: {conn['pid']})")
```

## Error Handling

All modules handle common exceptions:

```python
try:
    process = proc_mgr.get_process_by_pid(invalid_pid)
except psutil.NoSuchProcess:
    print("Process not found")
except psutil.AccessDenied:
    print("Permission denied")
```

## Thread Safety

- SystemMonitor: Thread-safe for read operations
- ProcessManager: Thread-safe for read operations
- NetworkManager: Thread-safe for read operations
- AIAssistant: Not thread-safe, create separate instances per thread

## Performance Considerations

- System monitoring has ~2% CPU overhead
- Process listing can take 100-500ms with many processes
- Network connection listing requires elevated privileges on Windows
- AI requests typically take 1-5 seconds depending on the provider

## Requirements

- Python 3.8+
- psutil >= 5.9.0
- PyQt6 >= 6.4.0 (for GUI)
- openai >= 1.0.0 (for AI features)
- anthropic >= 0.25.0 (alternative AI provider)
