"""
Process Manager Module
Manages and monitors system processes
"""

import psutil
from typing import Dict, List, Optional


class ProcessManager:
    """Manage and monitor system processes"""
    
    def __init__(self):
        pass
    
    def get_all_processes(self) -> List[Dict]:
        """Get information about all running processes"""
        processes = []
        for proc in psutil.process_iter(['pid', 'name', 'username', 'status', 'cpu_percent', 'memory_percent', 'memory_info', 'create_time']):
            try:
                pinfo = proc.info
                mem_info = pinfo.get('memory_info')
                if mem_info:
                    pinfo['memory_mb'] = round(mem_info.rss / (1024**2), 2)
                else:
                    pinfo['memory_mb'] = 0
                processes.append(pinfo)
            except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess):
                pass
        return processes
    
    def get_process_by_pid(self, pid: int) -> Optional[Dict]:
        """Get detailed information about a specific process"""
        try:
            proc = psutil.Process(pid)
            return {
                'pid': proc.pid,
                'name': proc.name(),
                'username': proc.username(),
                'status': proc.status(),
                'cpu_percent': proc.cpu_percent(interval=0.1),
                'memory_percent': proc.memory_percent(),
                'memory_mb': round(proc.memory_info().rss / (1024**2), 2),
                'create_time': proc.create_time(),
                'num_threads': proc.num_threads(),
                'cmdline': ' '.join(proc.cmdline()) if proc.cmdline() else '',
                'cwd': proc.cwd() if hasattr(proc, 'cwd') else None,
                'connections': len(proc.connections()) if hasattr(proc, 'connections') else 0
            }
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            return None
    
    def kill_process(self, pid: int) -> bool:
        """Kill a process by PID"""
        try:
            proc = psutil.Process(pid)
            proc.kill()
            return True
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            return False
    
    def terminate_process(self, pid: int) -> bool:
        """Gracefully terminate a process by PID"""
        try:
            proc = psutil.Process(pid)
            proc.terminate()
            return True
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            return False
    
    def get_process_tree(self, pid: int) -> Dict:
        """Get process tree for a given PID"""
        try:
            proc = psutil.Process(pid)
            children = proc.children(recursive=True)
            return {
                'parent': self.get_process_by_pid(pid),
                'children': [self.get_process_by_pid(child.pid) for child in children]
            }
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            return {'parent': None, 'children': []}
