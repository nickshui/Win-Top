"""
AI Assistant Module
Integrates AI capabilities to help users manage their Windows system
"""

import os
from typing import Dict, List, Optional
from dotenv import load_dotenv

# Load environment variables
load_dotenv()


class AIAssistant:
    """AI-powered assistant for Windows system management"""
    
    def __init__(self, provider: str = "openai"):
        """
        Initialize AI Assistant
        
        Args:
            provider: AI provider ('openai' or 'anthropic')
        """
        self.provider = provider
        self.client = None
        self._initialize_client()
    
    def _initialize_client(self):
        """Initialize the AI client based on provider"""
        if self.provider == "openai":
            try:
                import openai
                api_key = os.getenv("OPENAI_API_KEY")
                if api_key:
                    self.client = openai.OpenAI(api_key=api_key)
            except ImportError:
                print("OpenAI library not installed")
        elif self.provider == "anthropic":
            try:
                import anthropic
                api_key = os.getenv("ANTHROPIC_API_KEY")
                if api_key:
                    self.client = anthropic.Anthropic(api_key=api_key)
            except ImportError:
                print("Anthropic library not installed")
    
    def analyze_system_status(self, system_data: Dict) -> str:
        """
        Analyze system status and provide recommendations
        
        Args:
            system_data: Dictionary containing CPU, memory, disk, network info
            
        Returns:
            AI-generated analysis and recommendations
        """
        if not self.client:
            return "AI assistant not configured. Please set up API keys."
        
        prompt = f"""You are a Windows system management expert. Analyze the following system status and provide recommendations:

System Information:
- CPU Usage: {system_data.get('cpu', {}).get('total_usage', 'N/A')}%
- Memory Usage: {system_data.get('memory', {}).get('virtual', {}).get('percent', 'N/A')}%
- Disk Usage: {[f"{disk['device']}: {disk['percent']}%" for disk in system_data.get('disk', [])]}
- Network: Upload {system_data.get('network', {}).get('io_counters', {}).get('upload_speed_mbps', 'N/A')} MB/s, Download {system_data.get('network', {}).get('io_counters', {}).get('download_speed_mbps', 'N/A')} MB/s

Please provide:
1. Overall system health assessment
2. Potential issues or concerns
3. Optimization recommendations
4. Any immediate actions needed
"""
        
        try:
            return self._get_ai_response(prompt)
        except Exception as e:
            return f"Error getting AI analysis: {str(e)}"
    
    def diagnose_process(self, process_data: Dict) -> str:
        """
        Diagnose a specific process and provide insights
        
        Args:
            process_data: Dictionary containing process information
            
        Returns:
            AI-generated diagnosis and recommendations
        """
        if not self.client:
            return "AI assistant not configured. Please set up API keys."
        
        prompt = f"""Analyze this Windows process and provide insights:

Process: {process_data.get('name', 'Unknown')}
PID: {process_data.get('pid', 'N/A')}
CPU Usage: {process_data.get('cpu_percent', 'N/A')}%
Memory Usage: {process_data.get('memory_mb', 'N/A')} MB
Threads: {process_data.get('num_threads', 'N/A')}
Status: {process_data.get('status', 'N/A')}

Please provide:
1. What this process does
2. Whether this CPU/memory usage is normal
3. Any security concerns
4. Recommendations for management
"""
        
        try:
            return self._get_ai_response(prompt)
        except Exception as e:
            return f"Error getting process diagnosis: {str(e)}"
    
    def explain_windows_command(self, command: str) -> str:
        """
        Explain a Windows command and its usage
        
        Args:
            command: Windows command to explain
            
        Returns:
            AI-generated explanation
        """
        if not self.client:
            return "AI assistant not configured. Please set up API keys."
        
        prompt = f"""Explain the following Windows command in detail:

Command: {command}

Please provide:
1. What the command does
2. Common use cases
3. Important parameters and options
4. Examples of usage
5. Any precautions or warnings
"""
        
        try:
            return self._get_ai_response(prompt)
        except Exception as e:
            return f"Error explaining command: {str(e)}"
    
    def suggest_optimization(self, context: str) -> str:
        """
        Suggest system optimization based on context
        
        Args:
            context: Description of the issue or optimization goal
            
        Returns:
            AI-generated optimization suggestions
        """
        if not self.client:
            return "AI assistant not configured. Please set up API keys."
        
        prompt = f"""As a Windows system optimization expert, provide recommendations for:

{context}

Please provide specific, actionable steps to optimize the system.
"""
        
        try:
            return self._get_ai_response(prompt)
        except Exception as e:
            return f"Error getting optimization suggestions: {str(e)}"
    
    def answer_question(self, question: str, system_context: Optional[Dict] = None) -> str:
        """
        Answer general questions about Windows system management
        
        Args:
            question: User's question
            system_context: Optional system information for context
            
        Returns:
            AI-generated answer
        """
        if not self.client:
            return "AI assistant not configured. Please set up API keys."
        
        context_info = ""
        if system_context:
            context_info = f"\n\nCurrent System Context:\n{system_context}"
        
        prompt = f"""You are a Windows system management expert assistant. Answer the following question:

{question}{context_info}

Provide a clear, helpful answer with actionable advice when applicable.
"""
        
        try:
            return self._get_ai_response(prompt)
        except Exception as e:
            return f"Error answering question: {str(e)}"
    
    def _get_ai_response(self, prompt: str) -> str:
        """
        Get response from AI provider
        
        Args:
            prompt: The prompt to send to AI
            
        Returns:
            AI response text
        """
        if self.provider == "openai" and self.client:
            response = self.client.chat.completions.create(
                model="gpt-4",
                messages=[
                    {"role": "system", "content": "You are a Windows system management expert assistant."},
                    {"role": "user", "content": prompt}
                ],
                max_tokens=1000,
                temperature=0.7
            )
            return response.choices[0].message.content
        
        elif self.provider == "anthropic" and self.client:
            response = self.client.messages.create(
                model="claude-3-sonnet-20240229",
                max_tokens=1000,
                messages=[
                    {"role": "user", "content": prompt}
                ]
            )
            return response.content[0].text
        
        return "AI provider not properly configured."
