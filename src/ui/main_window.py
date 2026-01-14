"""
Main Window UI
Professional Windows resource management interface
"""

import sys
from PyQt6.QtWidgets import (
    QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QTabWidget, QLabel, QPushButton, QTextEdit, QTableWidget,
    QTableWidgetItem, QComboBox, QLineEdit, QMessageBox, QProgressBar,
    QSplitter, QGroupBox, QScrollArea
)
from PyQt6.QtCore import QTimer, Qt
from PyQt6.QtGui import QFont

from ..modules import SystemMonitor, ProcessManager, NetworkManager, AIAssistant
from ..modules.windows_commands import WindowsCommands
from ..utils import Config


class MainWindow(QMainWindow):
    """Main application window"""
    
    def __init__(self):
        super().__init__()
        self.system_monitor = SystemMonitor()
        self.process_manager = ProcessManager()
        self.network_manager = NetworkManager()
        self.ai_assistant = AIAssistant()
        self.windows_commands = WindowsCommands()
        
        self.init_ui()
        
        # Setup auto-refresh timer
        self.timer = QTimer()
        self.timer.timeout.connect(self.refresh_data)
        self.timer.start(Config.REFRESH_INTERVAL)  # Use config value
    
    def init_ui(self):
        """Initialize the user interface"""
        self.setWindowTitle("Win-Top - Windows Resource Management Tool")
        self.setGeometry(100, 100, 1400, 900)
        
        # Create central widget and main layout
        central_widget = QWidget()
        self.setCentralWidget(central_widget)
        main_layout = QVBoxLayout(central_widget)
        
        # Create tab widget
        tabs = QTabWidget()
        
        # Add tabs
        tabs.addTab(self.create_system_tab(), "System Monitor")
        tabs.addTab(self.create_process_tab(), "Process Manager")
        tabs.addTab(self.create_network_tab(), "Network Manager")
        tabs.addTab(self.create_commands_tab(), "Windows Commands")
        tabs.addTab(self.create_ai_tab(), "AI Assistant")
        
        main_layout.addWidget(tabs)
        
        # Status bar
        self.statusBar().showMessage("Ready")
    
    def create_system_tab(self):
        """Create system monitoring tab"""
        widget = QWidget()
        layout = QVBoxLayout(widget)
        
        # CPU Group
        cpu_group = QGroupBox("CPU Information")
        cpu_layout = QVBoxLayout()
        self.cpu_label = QLabel("CPU Usage: --")
        self.cpu_label.setFont(QFont("Arial", 12))
        self.cpu_progress = QProgressBar()
        cpu_layout.addWidget(self.cpu_label)
        cpu_layout.addWidget(self.cpu_progress)
        cpu_group.setLayout(cpu_layout)
        
        # Memory Group
        memory_group = QGroupBox("Memory Information")
        memory_layout = QVBoxLayout()
        self.memory_label = QLabel("Memory Usage: --")
        self.memory_label.setFont(QFont("Arial", 12))
        self.memory_progress = QProgressBar()
        memory_layout.addWidget(self.memory_label)
        memory_layout.addWidget(self.memory_progress)
        memory_group.setLayout(memory_layout)
        
        # Disk Group
        disk_group = QGroupBox("Disk Information")
        disk_layout = QVBoxLayout()
        self.disk_table = QTableWidget()
        self.disk_table.setColumnCount(5)
        self.disk_table.setHorizontalHeaderLabels(["Drive", "Total (GB)", "Used (GB)", "Free (GB)", "Usage (%)"])
        disk_layout.addWidget(self.disk_table)
        disk_group.setLayout(disk_layout)
        
        # Network Group
        network_group = QGroupBox("Network Information")
        network_layout = QVBoxLayout()
        self.network_label = QLabel("Network: --")
        self.network_label.setFont(QFont("Arial", 10))
        network_layout.addWidget(self.network_label)
        network_group.setLayout(network_layout)
        
        layout.addWidget(cpu_group)
        layout.addWidget(memory_group)
        layout.addWidget(disk_group)
        layout.addWidget(network_group)
        
        return widget
    
    def create_process_tab(self):
        """Create process management tab"""
        widget = QWidget()
        layout = QVBoxLayout(widget)
        
        # Buttons
        button_layout = QHBoxLayout()
        refresh_btn = QPushButton("Refresh")
        refresh_btn.clicked.connect(self.refresh_processes)
        kill_btn = QPushButton("Kill Selected Process")
        kill_btn.clicked.connect(self.kill_selected_process)
        button_layout.addWidget(refresh_btn)
        button_layout.addWidget(kill_btn)
        button_layout.addStretch()
        
        # Process table
        self.process_table = QTableWidget()
        self.process_table.setColumnCount(6)
        self.process_table.setHorizontalHeaderLabels(["PID", "Name", "Status", "CPU %", "Memory (MB)", "Username"])
        self.process_table.setSelectionBehavior(QTableWidget.SelectionBehavior.SelectRows)
        
        layout.addLayout(button_layout)
        layout.addWidget(self.process_table)
        
        return widget
    
    def create_network_tab(self):
        """Create network management tab"""
        widget = QWidget()
        layout = QVBoxLayout(widget)
        
        # Buttons
        button_layout = QHBoxLayout()
        refresh_btn = QPushButton("Refresh Connections")
        refresh_btn.clicked.connect(self.refresh_network)
        button_layout.addWidget(refresh_btn)
        button_layout.addStretch()
        
        # Network connections table
        self.network_table = QTableWidget()
        self.network_table.setColumnCount(6)
        self.network_table.setHorizontalHeaderLabels(["Protocol", "Local Address", "Remote Address", "Status", "PID", "Process"])
        
        layout.addLayout(button_layout)
        layout.addWidget(self.network_table)
        
        return widget
    
    def create_commands_tab(self):
        """Create Windows commands tab"""
        widget = QWidget()
        layout = QVBoxLayout(widget)
        
        # Command selector
        command_layout = QHBoxLayout()
        command_layout.addWidget(QLabel("Command:"))
        self.command_combo = QComboBox()
        self.command_combo.addItems(list(self.windows_commands.get_available_commands().keys()))
        command_layout.addWidget(self.command_combo)
        
        self.command_input = QLineEdit()
        self.command_input.setPlaceholderText("Additional parameters...")
        command_layout.addWidget(self.command_input)
        
        execute_btn = QPushButton("Execute")
        execute_btn.clicked.connect(self.execute_windows_command)
        command_layout.addWidget(execute_btn)
        
        # Output area
        self.command_output = QTextEdit()
        self.command_output.setReadOnly(True)
        self.command_output.setFont(QFont("Courier", 9))
        
        layout.addLayout(command_layout)
        layout.addWidget(QLabel("Output:"))
        layout.addWidget(self.command_output)
        
        return widget
    
    def create_ai_tab(self):
        """Create AI assistant tab"""
        widget = QWidget()
        layout = QVBoxLayout(widget)
        
        # AI Actions
        action_layout = QHBoxLayout()
        analyze_system_btn = QPushButton("Analyze System")
        analyze_system_btn.clicked.connect(self.ai_analyze_system)
        action_layout.addWidget(analyze_system_btn)
        action_layout.addStretch()
        
        # Question input
        question_layout = QHBoxLayout()
        question_layout.addWidget(QLabel("Ask AI:"))
        self.ai_question_input = QLineEdit()
        self.ai_question_input.setPlaceholderText("Ask a question about your Windows system...")
        question_layout.addWidget(self.ai_question_input)
        
        ask_btn = QPushButton("Ask")
        ask_btn.clicked.connect(self.ai_answer_question)
        question_layout.addWidget(ask_btn)
        
        # AI response area
        self.ai_output = QTextEdit()
        self.ai_output.setReadOnly(True)
        self.ai_output.setFont(QFont("Arial", 10))
        
        layout.addLayout(action_layout)
        layout.addLayout(question_layout)
        layout.addWidget(QLabel("AI Response:"))
        layout.addWidget(self.ai_output)
        
        # Info label
        info_label = QLabel("💡 The AI Assistant uses your system data to provide intelligent recommendations and answer questions.")
        info_label.setWordWrap(True)
        layout.addWidget(info_label)
        
        return widget
    
    def refresh_data(self):
        """Refresh all monitoring data"""
        # Update CPU
        cpu_info = self.system_monitor.get_cpu_info()
        cpu_usage = cpu_info['total_usage']
        self.cpu_label.setText(f"CPU Usage: {cpu_usage:.1f}% ({cpu_info['count']} cores)")
        self.cpu_progress.setValue(int(cpu_usage))
        
        # Update Memory
        memory_info = self.system_monitor.get_memory_info()
        memory_usage = memory_info['virtual']['percent']
        used_gb = memory_info['virtual']['used_gb']
        total_gb = memory_info['virtual']['total_gb']
        self.memory_label.setText(f"Memory Usage: {memory_usage:.1f}% ({used_gb:.1f}/{total_gb:.1f} GB)")
        self.memory_progress.setValue(int(memory_usage))
        
        # Update Disk
        disk_info = self.system_monitor.get_disk_info()
        self.disk_table.setRowCount(len(disk_info))
        for i, disk in enumerate(disk_info):
            self.disk_table.setItem(i, 0, QTableWidgetItem(disk['device']))
            self.disk_table.setItem(i, 1, QTableWidgetItem(str(disk['total_gb'])))
            self.disk_table.setItem(i, 2, QTableWidgetItem(str(disk['used_gb'])))
            self.disk_table.setItem(i, 3, QTableWidgetItem(str(disk['free_gb'])))
            self.disk_table.setItem(i, 4, QTableWidgetItem(f"{disk['percent']:.1f}"))
        
        # Update Network
        network_info = self.system_monitor.get_network_info()
        io_counters = network_info['io_counters']
        upload_speed = io_counters['upload_speed_mbps']
        download_speed = io_counters['download_speed_mbps']
        self.network_label.setText(
            f"Network I/O - Upload: {upload_speed:.2f} MB/s | Download: {download_speed:.2f} MB/s\n"
            f"Total Sent: {io_counters['bytes_sent_mb']:.1f} MB | Total Received: {io_counters['bytes_recv_mb']:.1f} MB"
        )
    
    def refresh_processes(self):
        """Refresh process list"""
        processes = self.process_manager.get_all_processes()
        self.process_table.setRowCount(len(processes))
        
        for i, proc in enumerate(processes):
            self.process_table.setItem(i, 0, QTableWidgetItem(str(proc.get('pid', ''))))
            self.process_table.setItem(i, 1, QTableWidgetItem(str(proc.get('name', ''))))
            self.process_table.setItem(i, 2, QTableWidgetItem(str(proc.get('status', ''))))
            self.process_table.setItem(i, 3, QTableWidgetItem(f"{proc.get('cpu_percent', 0):.1f}"))
            self.process_table.setItem(i, 4, QTableWidgetItem(str(proc.get('memory_mb', 0))))
            self.process_table.setItem(i, 5, QTableWidgetItem(str(proc.get('username', ''))))
    
    def kill_selected_process(self):
        """Kill the selected process"""
        selected_rows = self.process_table.selectedItems()
        if not selected_rows:
            QMessageBox.warning(self, "Warning", "Please select a process to kill")
            return
        
        pid_item = self.process_table.item(selected_rows[0].row(), 0)
        if not pid_item:
            QMessageBox.warning(self, "Error", "Could not get process information")
            return
        
        try:
            pid = int(pid_item.text())
        except (ValueError, AttributeError):
            QMessageBox.warning(self, "Error", "Invalid process ID")
            return
        
        reply = QMessageBox.question(
            self, "Confirm", f"Kill process with PID {pid}?",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No
        )
        
        if reply == QMessageBox.StandardButton.Yes:
            if self.process_manager.kill_process(pid):
                QMessageBox.information(self, "Success", f"Process {pid} killed successfully")
                self.refresh_processes()
            else:
                QMessageBox.warning(self, "Error", f"Failed to kill process {pid}")
    
    def refresh_network(self):
        """Refresh network connections"""
        connections = self.network_manager.get_connections()
        self.network_table.setRowCount(len(connections))
        
        for i, conn in enumerate(connections):
            self.network_table.setItem(i, 0, QTableWidgetItem(conn['type']))
            self.network_table.setItem(i, 1, QTableWidgetItem(conn['local_address']))
            self.network_table.setItem(i, 2, QTableWidgetItem(conn['remote_address']))
            self.network_table.setItem(i, 3, QTableWidgetItem(conn['status']))
            self.network_table.setItem(i, 4, QTableWidgetItem(str(conn.get('pid', ''))))
            self.network_table.setItem(i, 5, QTableWidgetItem(conn['process_name']))
    
    def execute_windows_command(self):
        """Execute selected Windows command"""
        command = self.command_combo.currentText()
        params = self.command_input.text()
        
        self.command_output.append(f"\n=== Executing: {command} {params} ===\n")
        
        # Map command to method
        command_map = {
            'ipconfig': lambda: self.windows_commands.ipconfig(params),
            'netstat': lambda: self.windows_commands.netstat(params if params else "-ano"),
            'tasklist': lambda: self.windows_commands.tasklist(params),
            'systeminfo': lambda: self.windows_commands.systeminfo(),
            'ping': lambda: self.windows_commands.ping(params if params else "8.8.8.8"),
            'tracert': lambda: self.windows_commands.tracert(params if params else "8.8.8.8"),
            'nslookup': lambda: self.windows_commands.nslookup(params if params else "google.com"),
            'wlan_profiles': lambda: self.windows_commands.netsh_wlan_show_profiles(),
            'disk_info': lambda: self.windows_commands.get_disk_info(),
            'flush_dns': lambda: self.windows_commands.flush_dns(),
            'route_table': lambda: self.windows_commands.get_route_table(),
            'arp_table': lambda: self.windows_commands.get_arp_table(),
            'services': lambda: self.windows_commands.get_services(),
            'firewall_status': lambda: self.windows_commands.get_firewall_status(),
        }
        
        if command in command_map:
            success, output = command_map[command]()
            self.command_output.append(output)
            if not success:
                self.command_output.append("\n[Command failed or returned error]")
        else:
            self.command_output.append(f"Command '{command}' not implemented")
    
    def ai_analyze_system(self):
        """Analyze system with AI"""
        self.ai_output.append("\n=== Analyzing System... ===\n")
        
        # Gather system data
        system_data = {
            'cpu': self.system_monitor.get_cpu_info(),
            'memory': self.system_monitor.get_memory_info(),
            'disk': self.system_monitor.get_disk_info(),
            'network': self.system_monitor.get_network_info()
        }
        
        # Get AI analysis
        analysis = self.ai_assistant.analyze_system_status(system_data)
        self.ai_output.append(analysis)
        self.ai_output.append("\n")
    
    def ai_answer_question(self):
        """Answer user question with AI"""
        question = self.ai_question_input.text().strip()
        if not question:
            QMessageBox.warning(self, "Warning", "Please enter a question")
            return
        
        self.ai_output.append(f"\n=== Question: {question} ===\n")
        
        # Gather system context
        system_context = {
            'cpu_usage': self.system_monitor.get_cpu_info()['total_usage'],
            'memory_usage': self.system_monitor.get_memory_info()['virtual']['percent']
        }
        
        # Get AI answer
        answer = self.ai_assistant.answer_question(question, system_context)
        self.ai_output.append(answer)
        self.ai_output.append("\n")
        
        self.ai_question_input.clear()
