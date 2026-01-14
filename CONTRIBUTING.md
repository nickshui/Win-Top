# Contributing to Win-Top

Thank you for your interest in contributing to Win-Top! 🎉

## How to Contribute

### Reporting Bugs

1. Check if the bug has already been reported in [Issues](https://github.com/nickshui/Win-Top/issues)
2. If not, create a new issue with:
   - Clear title and description
   - Steps to reproduce
   - Expected vs actual behavior
   - System information (OS, Python version)
   - Screenshots if applicable

### Suggesting Features

1. Check existing [Issues](https://github.com/nickshui/Win-Top/issues) for similar suggestions
2. Create a new issue with:
   - Clear description of the feature
   - Use case and benefits
   - Possible implementation approach

### Pull Requests

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/your-feature-name`
3. Make your changes
4. Test your changes thoroughly
5. Commit with clear messages: `git commit -m "Add feature: description"`
6. Push to your fork: `git push origin feature/your-feature-name`
7. Create a Pull Request

## Development Setup

```bash
# Clone your fork
git clone https://github.com/YOUR_USERNAME/Win-Top.git
cd Win-Top

# Install dependencies
pip install -r requirements.txt

# Install in development mode
pip install -e .

# Run tests (if available)
python -m pytest
```

## Code Style

- Follow PEP 8 style guide
- Use type hints where appropriate
- Add docstrings to functions and classes
- Keep functions focused and concise
- Comment complex logic

## Testing

- Test on Windows 10/11 if possible
- Verify all modules work independently
- Test UI changes with different screen sizes
- Check AI integration with both providers

## Areas for Contribution

### High Priority
- Unit tests for core modules
- Integration tests
- Performance optimization
- UI/UX improvements
- Documentation improvements

### Feature Ideas
- Additional system metrics
- More Windows commands
- Export data to CSV/JSON
- System alerts and notifications
- Historical data tracking
- Dark mode for UI
- Multi-language support
- Plugin system

### AI Enhancements
- Custom AI prompts
- Local LLM support
- AI training on system logs
- Predictive maintenance
- Automated troubleshooting

## Code Review Process

1. Maintainers will review your PR
2. Address any feedback or requested changes
3. Once approved, your PR will be merged
4. Your contribution will be credited

## Questions?

Feel free to open an issue for questions or discussion!

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

---

Thank you for making Win-Top better! 🚀
