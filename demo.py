#!/usr/bin/env python3
"""
Win-Top Demo Script
Demonstrates core functionality without GUI
"""

import sys
import time
sys.path.insert(0, '.')

from src.modules import SystemMonitor, ProcessManager, NetworkManager, AIAssistant
from src.modules.windows_commands import WindowsCommands


def print_separator(title=""):
    """Print a separator line"""
    if title:
        print(f"\n{'='*60}")
        print(f" {title}")
        print('='*60)
    else:
        print('-'*60)


def demo_system_monitor():
    """Demonstrate system monitoring"""
    print_separator("SYSTEM MONITORING")
    
    monitor = SystemMonitor()
    
    # CPU Information
    cpu = monitor.get_cpu_info()
    print(f"\n📊 CPU Information:")
    print(f"   Total Usage: {cpu['total_usage']:.1f}%")
    print(f"   Cores: {cpu['count']} logical, {cpu['physical_count']} physical")
    
    # Memory Information
    memory = monitor.get_memory_info()
    vm = memory['virtual']
    print(f"\n💾 Memory Information:")
    print(f"   Total: {vm['total_gb']:.2f} GB")
    print(f"   Used: {vm['used_gb']:.2f} GB ({vm['percent']:.1f}%)")
    print(f"   Available: {vm['available_gb']:.2f} GB")
    
    # Disk Information
    disks = monitor.get_disk_info()
    print(f"\n💿 Disk Information:")
    for disk in disks:
        print(f"   {disk['device']} ({disk['fstype']})")
        print(f"      Total: {disk['total_gb']:.1f} GB")
        print(f"      Used: {disk['used_gb']:.1f} GB ({disk['percent']:.1f}%)")
        print(f"      Free: {disk['free_gb']:.1f} GB")
    
    # Network Information
    network = monitor.get_network_info()
    io = network['io_counters']
    print(f"\n🌐 Network Information:")
    print(f"   Upload Speed: {io['upload_speed_mbps']:.2f} MB/s")
    print(f"   Download Speed: {io['download_speed_mbps']:.2f} MB/s")
    print(f"   Total Sent: {io['bytes_sent_mb']:.1f} MB")
    print(f"   Total Received: {io['bytes_recv_mb']:.1f} MB")


def demo_process_manager():
    """Demonstrate process management"""
    print_separator("PROCESS MANAGEMENT")
    
    proc_mgr = ProcessManager()
    
    # Get all processes
    processes = proc_mgr.get_all_processes()
    print(f"\n📋 Total Processes: {len(processes)}")
    
    # Sort by memory usage and show top 5
    sorted_procs = sorted(processes, key=lambda x: x.get('memory_mb', 0), reverse=True)
    print(f"\n🔝 Top 5 Processes by Memory Usage:")
    print(f"{'PID':<8} {'Name':<30} {'Memory (MB)':<12} {'CPU %':<8}")
    print_separator()
    for proc in sorted_procs[:5]:
        print(f"{proc.get('pid', 0):<8} {str(proc.get('name', ''))[:29]:<30} "
              f"{proc.get('memory_mb', 0):<12.1f} {proc.get('cpu_percent', 0):<8.1f}")


def demo_network_manager():
    """Demonstrate network management"""
    print_separator("NETWORK MANAGEMENT")
    
    net_mgr = NetworkManager()
    
    # Get network statistics
    stats = net_mgr.get_network_stats()
    print(f"\n🌐 Network Interfaces: {len(stats)}")
    for name, stat in stats.items():
        status = "UP" if stat['isup'] else "DOWN"
        print(f"   {name}: {status}, Speed: {stat['speed']} Mbps")
    
    # Get listening ports
    listening = net_mgr.get_listening_ports()
    print(f"\n👂 Listening Ports (showing first 10):")
    print(f"{'Port':<8} {'Address':<20} {'Process':<30} {'PID':<8}")
    print_separator()
    for conn in listening[:10]:
        pid_str = str(conn.get('pid', 'N/A')) if conn.get('pid') is not None else 'N/A'
        print(f"{conn['port']:<8} {conn['address']:<20} "
              f"{str(conn['process_name'])[:29]:<30} {pid_str:<8}")



def demo_windows_commands():
    """Demonstrate Windows commands"""
    print_separator("WINDOWS COMMANDS")
    
    cmd = WindowsCommands()
    
    # List available commands
    commands = cmd.get_available_commands()
    print(f"\n⚡ Available Commands: {len(commands)}")
    for name, desc in list(commands.items())[:8]:
        print(f"   {name:<20} - {desc}")
    
    # Execute a simple command
    print(f"\n🔧 Executing: ping (testing connectivity to 8.8.8.8)")
    success, output = cmd.ping("8.8.8.8", count=2)
    if success:
        lines = output.split('\n')[:5]  # Show first 5 lines
        for line in lines:
            if line.strip():
                print(f"   {line}")


def demo_ai_assistant():
    """Demonstrate AI assistant"""
    print_separator("AI ASSISTANT")
    
    ai = AIAssistant()
    
    print(f"\n🤖 AI Assistant Status:")
    if not ai.client:
        print("   ⚠️  AI not configured (no API key)")
        print("   To enable AI features:")
        print("   1. Create a .env file")
        print("   2. Add: OPENAI_API_KEY=your_key_here")
        print("   3. Or: ANTHROPIC_API_KEY=your_key_here")
    else:
        print("   ✅ AI configured and ready")
        print("   Provider: " + ai.provider)
        
        # Demo AI capabilities
        print(f"\n💡 AI Capabilities:")
        print("   - System status analysis")
        print("   - Process diagnostics")
        print("   - Windows command explanations")
        print("   - Optimization suggestions")
        print("   - Natural language Q&A")


def main():
    """Main demo function"""
    print("\n" + "="*60)
    print(" Win-Top - Windows Resource Management Tool")
    print(" Demo Script (No GUI Required)")
    print("="*60)
    
    try:
        # Run all demos
        demo_system_monitor()
        time.sleep(0.5)
        
        demo_process_manager()
        time.sleep(0.5)
        
        demo_network_manager()
        time.sleep(0.5)
        
        demo_windows_commands()
        time.sleep(0.5)
        
        demo_ai_assistant()
        
        print_separator()
        print("\n✅ Demo completed successfully!")
        print("\n💡 To launch the full GUI application, run:")
        print("   python src/main.py")
        print("\n📖 For more information, see:")
        print("   - README.md")
        print("   - USAGE_GUIDE.md")
        print("="*60 + "\n")
        
    except KeyboardInterrupt:
        print("\n\n⚠️  Demo interrupted by user")
    except Exception as e:
        print(f"\n\n❌ Error: {e}")
        import traceback
        traceback.print_exc()


if __name__ == "__main__":
    main()
