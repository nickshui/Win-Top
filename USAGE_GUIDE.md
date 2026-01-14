# Win-Top 使用指南

## 快速开始

### 1. 首次运行
```bash
# 安装依赖
pip install -r requirements.txt

# 运行应用
python src/main.py
```

### 2. 配置AI功能（推荐）

AI功能是Win-Top的核心特性，强烈建议配置：

#### 获取OpenAI API密钥
1. 访问 https://platform.openai.com/
2. 注册/登录账户
3. 创建API密钥
4. 复制密钥到 `.env` 文件

#### 获取Anthropic API密钥
1. 访问 https://console.anthropic.com/
2. 注册/登录账户
3. 创建API密钥
4. 复制密钥到 `.env` 文件

#### 配置文件示例
```env
OPENAI_API_KEY=sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
# 或者
ANTHROPIC_API_KEY=sk-ant-xxxxxxxxxxxxxxxxxxxxxxxx

AI_PROVIDER=openai  # 或 anthropic
```

## 功能详细说明

### 系统监控标签

#### CPU监控
- 实时显示总体CPU使用率
- 显示每个CPU核心的使用情况
- CPU频率信息
- 进度条可视化

#### 内存监控
- 虚拟内存使用情况
- 交换空间使用情况
- GB单位显示
- 使用百分比

#### 磁盘监控
- 所有磁盘分区列表
- 每个分区的总容量、已用空间、剩余空间
- 使用百分比
- 自动更新

#### 网络监控
- 实时上传/下载速度
- 累计发送/接收流量
- MB/s单位显示

### 进程管理标签

#### 查看进程
- PID（进程ID）
- 进程名称
- 运行状态
- CPU使用率
- 内存使用量
- 用户名

#### 管理进程
1. 点击选择要管理的进程
2. 点击"Kill Selected Process"终止进程
3. 确认对话框中选择"Yes"
4. 进程将被终止

⚠️ **注意**: 终止系统进程可能导致系统不稳定

### 网络管理标签

#### 网络连接列表
- 协议类型（TCP/UDP）
- 本地地址和端口
- 远程地址和端口
- 连接状态
- 关联的进程

#### 使用场景
- 查找占用特定端口的程序
- 监控网络连接
- 识别可疑连接
- 排查网络问题

### Windows命令标签

#### 可用命令

##### ipconfig
查看网络配置
```
参数: /all - 显示完整配置
```

##### netstat
网络连接统计
```
默认参数: -ano
参数: -an, -ano, -r, -s
```

##### tasklist
查看进程列表
```
参数: /FI "IMAGENAME eq chrome.exe"
```

##### systeminfo
系统信息
```
无需参数
```

##### ping
测试网络连通性
```
参数: google.com, 8.8.8.8
```

##### tracert
路由跟踪
```
参数: google.com
```

##### nslookup
DNS查询
```
参数: google.com, github.com
```

##### 其他命令
- wlan_profiles: 查看WiFi配置
- flush_dns: 刷新DNS缓存
- route_table: 路由表
- arp_table: ARP缓存
- services: Windows服务
- firewall_status: 防火墙状态

### AI助手标签

#### 系统分析
1. 点击"Analyze System"按钮
2. AI将分析当前系统状态
3. 提供健康评估
4. 给出优化建议
5. 识别潜在问题

#### 智能问答
在"Ask AI"输入框中输入问题，例如：

##### 系统性能问题
- "为什么我的电脑运行缓慢？"
- "如何提高系统性能？"
- "内存占用太高怎么办？"

##### 进程问题
- "svchost.exe是什么进程？"
- "chrome.exe占用CPU太高正常吗？"
- "如何找出哪个程序在占用网络？"

##### 网络问题
- "如何测试网络连接？"
- "为什么网速变慢了？"
- "如何查看哪个程序在下载东西？"

##### 系统管理
- "如何清理磁盘空间？"
- "如何设置防火墙规则？"
- "如何查看系统日志？"

#### AI功能特点
- ✅ 基于实际系统数据分析
- ✅ 提供具体可操作的建议
- ✅ 支持中英文问答
- ✅ 上下文感知
- ✅ 专业的系统管理知识

## 高级技巧

### 1. 性能优化
- 关闭不需要的自动刷新
- 减少显示的进程数量
- 使用过滤功能

### 2. 安全使用
- 不要随意终止系统进程
- 检查AI建议的合理性
- 在执行命令前了解其作用

### 3. 故障排除

#### AI功能无响应
- 检查.env文件配置
- 验证API密钥有效性
- 检查网络连接
- 查看命令行输出的错误信息

#### 程序无法启动
- 检查Python版本 (需要3.8+)
- 重新安装依赖: `pip install -r requirements.txt`
- 检查PyQt6安装: `pip install PyQt6`

#### 权限不足
- 以管理员身份运行
- 右键点击程序 → "以管理员身份运行"

## 最佳实践

1. **定期监控**: 定期查看系统状态，预防问题
2. **善用AI**: 遇到问题时先咨询AI助手
3. **谨慎操作**: 终止进程前先了解其作用
4. **保持更新**: 定期更新Win-Top和依赖库

## 常见问题

### Q: AI功能需要收费吗？
A: AI功能使用OpenAI或Anthropic的API，需要根据使用量付费。建议查看各自的定价政策。

### Q: 可以在其他操作系统上使用吗？
A: Win-Top主要为Windows设计，但大部分功能在Linux和macOS上也能运行。Windows命令功能仅在Windows上可用。

### Q: 如何提高AI响应速度？
A: 可以在配置中选择更快的模型，或者使用更简洁的问题描述。

### Q: 数据安全吗？
A: Win-Top不收集或上传任何个人数据。AI功能仅在您主动使用时才会发送必要的系统信息到AI服务提供商。

## 获取帮助

- GitHub Issues: https://github.com/nickshui/Win-Top/issues
- 使用AI助手: 在应用中直接询问问题
- 查看日志: 命令行输出包含详细信息

---

祝使用愉快！ 🎉
