from setuptools import setup, find_packages

setup(
    name="Win-Top",
    version="1.0.0",
    description="A professional Windows resource management tool with AI integration",
    author="Win-Top Team",
    packages=find_packages(),
    install_requires=[
        'psutil>=5.9.0',
        'PyQt6>=6.4.0',
        'openai>=1.0.0',
        'anthropic>=0.25.0',
        'requests>=2.31.0',
        'python-dotenv>=1.0.0',
    ],
    entry_points={
        'console_scripts': [
            'win-top=src.main:main',
        ],
    },
    python_requires='>=3.8',
)
