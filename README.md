# Win-Top - Windows资源管理工具 🚀

一个专业、现代、美观的Windows资源管理工具，集成AI能力，帮助用户更好地管理Windows电脑。

## ✨ 主要特性

### 核心功能
- 🖥️ **系统监控**: 实时监控CPU、内存、磁盘使用情况
- 📊 **进程管理**: 查看和管理系统进程，包括终止进程功能
- 🌐 **网络管理**: 监控网络连接、端口、IP地址和流量
- 💾 **磁盘监控**: 查看所有磁盘分区的使用情况
- ⚡ **Windows命令集成**: 内置常用Windows命令工具，一键执行

### 🤖 AI功能（核心特性）
- **智能系统分析**: AI分析系统状态，提供优化建议
- **进程诊断**: AI诊断进程行为，识别潜在问题
- **命令解释**: AI解释Windows命令的用途和使用方法
- **智能问答**: 通过自然语言与AI交互，获取系统管理建议
- **优化建议**: 基于系统状态的智能优化推荐

## 📦 安装

### 环境要求
- Python 3.8+
- Windows 10/11 (推荐)

### 安装步骤

1. 克隆仓库:
```bash
git clone https://github.com/nickshui/Win-Top.git
cd Win-Top
```

2. 安装依赖:
```bash
pip install -r requirements.txt
```

3. 配置AI (可选，推荐):

创建 `.env` 文件并添加AI API密钥:
```env
# OpenAI配置
OPENAI_API_KEY=your_openai_api_key_here

# 或者使用Anthropic Claude
ANTHROPIC_API_KEY=your_anthropic_api_key_here
```

4. 运行应用:
```bash
python src/main.py
```

或者使用安装模式:
```bash
pip install -e .
win-top
```

## 🎯 功能详解

### 1. 系统监控
- 实时显示CPU使用率（总体和每个核心）
- 内存使用情况（虚拟内存和交换空间）
- 所有磁盘分区的使用统计
- 网络上传/下载速度和流量统计

### 2. 进程管理
- 查看所有运行中的进程
- 显示进程的PID、名称、状态、CPU和内存使用
- 终止选中的进程
- 按资源使用排序

### 3. 网络管理
- 查看所有网络连接
- 显示本地和远程地址
- 识别连接对应的进程
- 监控端口使用情况

### 4. Windows命令工具
内置以下常用命令:
- `ipconfig` - 网络配置
- `netstat` - 网络连接统计
- `tasklist` - 进程列表
- `systeminfo` - 系统信息
- `ping` - 网络连通性测试
- `tracert` - 路由跟踪
- `nslookup` - DNS查询
- `wlan_profiles` - WiFi配置文件
- `flush_dns` - 刷新DNS缓存
- `route_table` - 路由表
- `arp_table` - ARP缓存
- `services` - Windows服务
- `firewall_status` - 防火墙状态

### 5. 🤖 AI助手
Win-Top的核心特性，提供智能系统管理辅助:

#### AI功能示例:
- **系统分析**: "分析我的系统健康状况" → AI提供综合评估和优化建议
- **问题诊断**: "为什么我的电脑这么慢？" → AI分析资源使用并给出原因
- **进程识别**: "svchost.exe是什么？" → AI解释系统进程的作用
- **优化建议**: "如何减少内存使用？" → AI提供具体的优化步骤
- **命令帮助**: "netstat命令怎么用？" → AI详细解释命令用法

## 🔧 技术架构

### 核心技术栈
- **UI框架**: PyQt6 - 现代化的跨平台GUI
- **系统监控**: psutil - 跨平台系统和进程工具
- **AI集成**: 
  - OpenAI GPT-4 - 强大的自然语言理解和生成
  - Anthropic Claude - 替代AI提供商
- **命令执行**: subprocess - 安全的系统命令执行

### 模块结构
```
src/
├── main.py                 # 应用入口
├── modules/
│   ├── system_monitor.py   # 系统监控模块
│   ├── process_manager.py  # 进程管理模块
│   ├── network_manager.py  # 网络管理模块
│   ├── ai_assistant.py     # AI助手模块
│   └── windows_commands.py # Windows命令模块
└── ui/
    └── main_window.py      # 主界面
```

## 🎨 界面预览

Win-Top提供清晰、直观的多标签界面:
1. **系统监控** - 实时资源使用仪表板
2. **进程管理** - 进程列表和管理工具
3. **网络管理** - 网络连接和统计信息
4. **Windows命令** - 内置命令工具
5. **AI助手** - 智能系统管理助手

## 🚀 使用示例

### 基本使用
1. 启动Win-Top
2. 查看"系统监控"标签了解资源使用
3. 在"进程管理"中管理应用程序
4. 使用"AI助手"获取智能建议

### AI助手使用
1. 点击"Analyze System"按钮进行系统分析
2. 在输入框输入问题，如："如何优化系统性能？"
3. AI将基于当前系统状态提供建议

### 执行Windows命令
1. 切换到"Windows Commands"标签
2. 从下拉菜单选择命令
3. 输入参数（如果需要）
4. 点击"Execute"执行

## ⚠️ 注意事项

1. **权限**: 某些操作（如终止进程、某些系统命令）需要管理员权限
2. **AI配置**: AI功能需要配置有效的API密钥
3. **系统兼容**: 主要为Windows系统设计，部分功能在其他平台可能不可用
4. **资源使用**: 实时监控会占用一定系统资源

## 🤝 贡献

欢迎贡献代码、报告问题或提出新功能建议！

## 📄 许可证

本项目采用MIT许可证。

## 🙏 致谢

- [psutil](https://github.com/giampaolo/psutil) - 系统监控库
- [PyQt6](https://www.riverbankcomputing.com/software/pyqt/) - GUI框架
- [OpenAI](https://openai.com/) - AI能力支持
- [Anthropic](https://www.anthropic.com/) - Claude AI支持

---

**Win-Top** - 让Windows管理更智能、更简单！ 🎉
