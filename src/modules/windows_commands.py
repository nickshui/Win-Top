"""
Windows Commands Module
Provides built-in Windows command execution utilities
"""

import subprocess
import platform
from typing import Dict, Tuple, List


class WindowsCommands:
    """Execute and manage common Windows commands"""
    
    def __init__(self):
        self.is_windows = platform.system() == 'Windows'
    
    def _execute_command(self, command: List[str]) -> Tuple[bool, str]:
        """
        Execute a command and return result
        
        Args:
            command: Command as list of strings
            
        Returns:
            Tuple of (success, output)
        """
        try:
            result = subprocess.run(
                command,
                capture_output=True,
                text=True,
                shell=True if self.is_windows else False,
                timeout=30
            )
            return True, result.stdout if result.returncode == 0 else result.stderr
        except subprocess.TimeoutExpired:
            return False, "Command timed out"
        except Exception as e:
            return False, f"Error: {str(e)}"
    
    def ipconfig(self, option: str = "") -> Tuple[bool, str]:
        """Execute ipconfig command"""
        cmd = ["ipconfig"]
        if option:
            cmd.append(option)
        return self._execute_command(cmd)
    
    def netstat(self, option: str = "-ano") -> Tuple[bool, str]:
        """Execute netstat command"""
        return self._execute_command(["netstat", option])
    
    def tasklist(self, filter_name: str = "") -> Tuple[bool, str]:
        """Execute tasklist command"""
        cmd = ["tasklist"]
        if filter_name:
            cmd.extend(["/FI", f"IMAGENAME eq {filter_name}"])
        return self._execute_command(cmd)
    
    def taskkill(self, pid: int = None, name: str = None, force: bool = True) -> Tuple[bool, str]:
        """Execute taskkill command"""
        cmd = ["taskkill"]
        if force:
            cmd.append("/F")
        if pid:
            cmd.extend(["/PID", str(pid)])
        elif name:
            cmd.extend(["/IM", name])
        else:
            return False, "Must specify either PID or process name"
        return self._execute_command(cmd)
    
    def systeminfo(self) -> Tuple[bool, str]:
        """Execute systeminfo command"""
        return self._execute_command(["systeminfo"])
    
    def ping(self, host: str, count: int = 4) -> Tuple[bool, str]:
        """Execute ping command"""
        cmd = ["ping"]
        if self.is_windows:
            cmd.extend(["-n", str(count), host])
        else:
            cmd.extend(["-c", str(count), host])
        return self._execute_command(cmd)
    
    def tracert(self, host: str) -> Tuple[bool, str]:
        """Execute tracert/traceroute command"""
        cmd = ["tracert" if self.is_windows else "traceroute", host]
        return self._execute_command(cmd)
    
    def nslookup(self, host: str) -> Tuple[bool, str]:
        """Execute nslookup command"""
        return self._execute_command(["nslookup", host])
    
    def netsh_wlan_show_profiles(self) -> Tuple[bool, str]:
        """Show WLAN profiles"""
        return self._execute_command(["netsh", "wlan", "show", "profiles"])
    
    def get_disk_info(self) -> Tuple[bool, str]:
        """Get disk information using wmic or similar"""
        if self.is_windows:
            return self._execute_command(["wmic", "logicaldisk", "get", "name,size,freespace"])
        else:
            return self._execute_command(["df", "-h"])
    
    def flush_dns(self) -> Tuple[bool, str]:
        """Flush DNS cache"""
        if self.is_windows:
            return self._execute_command(["ipconfig", "/flushdns"])
        else:
            return False, "Command only available on Windows"
    
    def get_route_table(self) -> Tuple[bool, str]:
        """Get routing table"""
        if self.is_windows:
            return self._execute_command(["route", "print"])
        else:
            return self._execute_command(["route", "-n"])
    
    def get_arp_table(self) -> Tuple[bool, str]:
        """Get ARP table"""
        return self._execute_command(["arp", "-a"])
    
    def check_disk(self, drive: str = "C:") -> Tuple[bool, str]:
        """Check disk for errors (requires admin)"""
        if self.is_windows:
            return self._execute_command(["chkdsk", drive])
        else:
            return False, "Command only available on Windows"
    
    def get_services(self) -> Tuple[bool, str]:
        """Get list of Windows services"""
        if self.is_windows:
            return self._execute_command(["sc", "query"])
        else:
            return self._execute_command(["systemctl", "list-units", "--type=service"])
    
    def get_firewall_status(self) -> Tuple[bool, str]:
        """Get Windows Firewall status"""
        if self.is_windows:
            return self._execute_command(["netsh", "advfirewall", "show", "allprofiles"])
        else:
            return False, "Command only available on Windows"
    
    def get_available_commands(self) -> Dict[str, str]:
        """Get list of available commands with descriptions"""
        return {
            'ipconfig': 'Display network configuration',
            'netstat': 'Display network connections and statistics',
            'tasklist': 'Display running processes',
            'taskkill': 'Terminate a process',
            'systeminfo': 'Display system information',
            'ping': 'Test network connectivity',
            'tracert': 'Trace route to a host',
            'nslookup': 'Query DNS records',
            'wlan_profiles': 'Show saved WiFi profiles',
            'disk_info': 'Display disk information',
            'flush_dns': 'Flush DNS resolver cache',
            'route_table': 'Display routing table',
            'arp_table': 'Display ARP cache',
            'check_disk': 'Check disk for errors',
            'services': 'List Windows services',
            'firewall_status': 'Display firewall status'
        }
