"""
Network Manager Module
Manages network connections, ports, and monitoring
"""

import psutil
from typing import Dict, List


class NetworkManager:
    """Manage network connections and ports"""
    
    def __init__(self):
        pass
    
    def get_connections(self, kind: str = 'inet') -> List[Dict]:
        """Get all network connections"""
        connections = []
        for conn in psutil.net_connections(kind=kind):
            try:
                process_name = psutil.Process(conn.pid).name() if conn.pid else 'System'
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                process_name = 'Unknown'
            
            connections.append({
                'fd': conn.fd,
                'family': str(conn.family),
                'type': str(conn.type),
                'local_address': f"{conn.laddr.ip}:{conn.laddr.port}" if conn.laddr else '',
                'remote_address': f"{conn.raddr.ip}:{conn.raddr.port}" if conn.raddr else '',
                'status': conn.status,
                'pid': conn.pid,
                'process_name': process_name
            })
        return connections
    
    def get_listening_ports(self) -> List[Dict]:
        """Get all listening ports"""
        listening = []
        for conn in psutil.net_connections(kind='inet'):
            if conn.status == psutil.CONN_LISTEN:
                try:
                    process_name = psutil.Process(conn.pid).name() if conn.pid else 'System'
                except (psutil.NoSuchProcess, psutil.AccessDenied):
                    process_name = 'Unknown'
                
                listening.append({
                    'port': conn.laddr.port if conn.laddr else 0,
                    'address': conn.laddr.ip if conn.laddr else '',
                    'pid': conn.pid,
                    'process_name': process_name,
                    'protocol': str(conn.type)
                })
        return listening
    
    def get_connections_by_pid(self, pid: int) -> List[Dict]:
        """Get network connections for a specific process"""
        connections = []
        try:
            proc = psutil.Process(pid)
            for conn in proc.connections():
                connections.append({
                    'fd': conn.fd,
                    'family': str(conn.family),
                    'type': str(conn.type),
                    'local_address': f"{conn.laddr.ip}:{conn.laddr.port}" if conn.laddr else '',
                    'remote_address': f"{conn.raddr.ip}:{conn.raddr.port}" if conn.raddr else '',
                    'status': conn.status
                })
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
        return connections
    
    def get_network_stats(self) -> Dict:
        """Get network statistics"""
        stats = psutil.net_if_stats()
        return {
            name: {
                'isup': stat.isup,
                'duplex': str(stat.duplex),
                'speed': stat.speed,
                'mtu': stat.mtu
            }
            for name, stat in stats.items()
        }
