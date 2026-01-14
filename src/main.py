"""
Main Entry Point
Win-Top - Windows Resource Management Tool with AI Integration
"""

import sys
from PyQt6.QtWidgets import QApplication
from src.ui.main_window import MainWindow


def main():
    """Main application entry point"""
    app = QApplication(sys.argv)
    
    # Set application style
    app.setStyle('Fusion')
    
    # Create and show main window
    window = MainWindow()
    window.show()
    
    # Run application
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
