"""
Configuration Manager
Handles application configuration and settings
"""

import os
from typing import Dict, Any
from dotenv import load_dotenv

load_dotenv()


class Config:
    """Application configuration manager"""
    
    # AI Configuration
    AI_PROVIDER = os.getenv('AI_PROVIDER', 'openai')
    OPENAI_API_KEY = os.getenv('OPENAI_API_KEY', '')
    ANTHROPIC_API_KEY = os.getenv('ANTHROPIC_API_KEY', '')
    
    # Application Settings
    REFRESH_INTERVAL = int(os.getenv('REFRESH_INTERVAL', '2000'))
    
    # UI Settings
    WINDOW_WIDTH = 1400
    WINDOW_HEIGHT = 900
    
    @classmethod
    def is_ai_configured(cls) -> bool:
        """Check if AI is properly configured"""
        if cls.AI_PROVIDER == 'openai':
            return bool(cls.OPENAI_API_KEY)
        elif cls.AI_PROVIDER == 'anthropic':
            return bool(cls.ANTHROPIC_API_KEY)
        return False
    
    @classmethod
    def get_config(cls) -> Dict[str, Any]:
        """Get all configuration as dictionary"""
        return {
            'ai_provider': cls.AI_PROVIDER,
            'ai_configured': cls.is_ai_configured(),
            'refresh_interval': cls.REFRESH_INTERVAL,
            'window_width': cls.WINDOW_WIDTH,
            'window_height': cls.WINDOW_HEIGHT
        }
