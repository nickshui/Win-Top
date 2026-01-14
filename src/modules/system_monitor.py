"""
System Monitor Module
Monitors CPU, Memory, Disk, and Network resources
"""

import psutil
import time
from typing import Dict, List


class SystemMonitor:
    """Monitor system resources including CPU, Memory, Disk, and Network"""
    
    def __init__(self):
        self.previous_net_io = psutil.net_io_counters()
        self.previous_time = time.time()
    
    def get_cpu_info(self) -> Dict:
        """Get CPU usage information"""
        cpu_percent = psutil.cpu_percent(interval=1, percpu=True)
        total_usage = sum(cpu_percent) / len(cpu_percent) if cpu_percent else 0
        return {
            'total_usage': total_usage,
            'per_cpu': cpu_percent,
            'count': psutil.cpu_count(logical=True),
            'physical_count': psutil.cpu_count(logical=False),
            'frequency': psutil.cpu_freq()._asdict() if psutil.cpu_freq() else {}
        }
    
    def get_memory_info(self) -> Dict:
        """Get memory usage information"""
        virtual_mem = psutil.virtual_memory()
        swap_mem = psutil.swap_memory()
        
        return {
            'virtual': {
                'total': virtual_mem.total,
                'available': virtual_mem.available,
                'used': virtual_mem.used,
                'percent': virtual_mem.percent,
                'total_gb': round(virtual_mem.total / (1024**3), 2),
                'used_gb': round(virtual_mem.used / (1024**3), 2),
                'available_gb': round(virtual_mem.available / (1024**3), 2)
            },
            'swap': {
                'total': swap_mem.total,
                'used': swap_mem.used,
                'percent': swap_mem.percent,
                'total_gb': round(swap_mem.total / (1024**3), 2),
                'used_gb': round(swap_mem.used / (1024**3), 2)
            }
        }
    
    def get_disk_info(self) -> List[Dict]:
        """Get disk usage information"""
        disks = []
        for partition in psutil.disk_partitions():
            try:
                usage = psutil.disk_usage(partition.mountpoint)
                disks.append({
                    'device': partition.device,
                    'mountpoint': partition.mountpoint,
                    'fstype': partition.fstype,
                    'total': usage.total,
                    'used': usage.used,
                    'free': usage.free,
                    'percent': usage.percent,
                    'total_gb': round(usage.total / (1024**3), 2),
                    'used_gb': round(usage.used / (1024**3), 2),
                    'free_gb': round(usage.free / (1024**3), 2)
                })
            except PermissionError:
                continue
        return disks
    
    def get_network_info(self) -> Dict:
        """Get network interface and IO information"""
        current_net_io = psutil.net_io_counters()
        current_time = time.time()
        
        time_delta = current_time - self.previous_time
        
        # Calculate speeds
        bytes_sent_per_sec = (current_net_io.bytes_sent - self.previous_net_io.bytes_sent) / time_delta
        bytes_recv_per_sec = (current_net_io.bytes_recv - self.previous_net_io.bytes_recv) / time_delta
        
        # Update previous values
        self.previous_net_io = current_net_io
        self.previous_time = current_time
        
        # Get network interfaces
        interfaces = []
        for name, addrs in psutil.net_if_addrs().items():
            interface_info = {'name': name, 'addresses': []}
            for addr in addrs:
                interface_info['addresses'].append({
                    'family': str(addr.family),
                    'address': addr.address,
                    'netmask': addr.netmask,
                    'broadcast': addr.broadcast
                })
            interfaces.append(interface_info)
        
        return {
            'io_counters': {
                'bytes_sent': current_net_io.bytes_sent,
                'bytes_recv': current_net_io.bytes_recv,
                'packets_sent': current_net_io.packets_sent,
                'packets_recv': current_net_io.packets_recv,
                'bytes_sent_mb': round(current_net_io.bytes_sent / (1024**2), 2),
                'bytes_recv_mb': round(current_net_io.bytes_recv / (1024**2), 2),
                'upload_speed_mbps': round(bytes_sent_per_sec / (1024**2), 2),
                'download_speed_mbps': round(bytes_recv_per_sec / (1024**2), 2)
            },
            'interfaces': interfaces
        }
