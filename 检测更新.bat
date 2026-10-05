@echo off
setlocal
chcp 65001 >nul 2>&1
title NetTrigger - 检查更新
cd /d "%~dp0."
set "SRC_FILE=%~f0"
set "TMP_PS=%TEMP%\nt_upd_%RANDOM%.ps1"
powershell -NoProfile -Command "Get-Content -LiteralPath $env:SRC_FILE -Encoding UTF8 | Select-Object -Skip 14 | Set-Content -LiteralPath $env:TMP_PS -Encoding UTF8"
powershell -NoProfile -ExecutionPolicy Bypass -File "%TMP_PS%"
if exist "%TMP_PS%" del "%TMP_PS%" >nul 2>&1
echo.
pause
exit /b 0

# ==============================================================================
# NetTrigger 独立更新检测与一键升级脚本
# 设计原则：完全独立运行、零后台驻留、零常驻性能损耗、断网安全自愈
# ==============================================================================

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

function Write-Color([string]$text, [ConsoleColor]$color = [ConsoleColor]::White, [switch]$NoNewline) {
    if ($NoNewline) {
        Write-Host $text -ForegroundColor $color -NoNewline
    } else {
        Write-Host $text -ForegroundColor $color
    }
}

function Show-Banner {
    Clear-Host
    Write-Color "=================================================================" -ForegroundColor Cyan
    Write-Color "           ⚡ NetTrigger 网络状态感知与守护引擎                  " -ForegroundColor Cyan
    Write-Color "                 独立自动检测更新与升级程序                      " -ForegroundColor White
    Write-Color "         (完全独立运行，不占用 NetTrigger 主程序任何常驻资源)    " -ForegroundColor DarkGray
    Write-Color "=================================================================" -ForegroundColor Cyan
    Write-Host ""
}

Show-Banner

# 1. 检测本地 NetTrigger 版本
$ScriptDir = (Get-Location).Path
$ExePath = Join-Path $ScriptDir "NetTrigger.exe"
if (-not (Test-Path -LiteralPath $ExePath)) {
    # 兼容便携安装与源码构建环境
    $candidates = @(
        (Join-Path $ScriptDir "NetTrigger.exe"),
        (Join-Path $ScriptDir "target\debug\NetTrigger.exe"),
        (Join-Path $ScriptDir "target\release\NetTrigger.exe")
    )
    foreach ($cand in $candidates) {
        if (Test-Path -LiteralPath $cand) {
            $ExePath = $cand
            break
        }
    }
}

$localVerStr = ""
$localVer = $null

if (Test-Path -LiteralPath $ExePath) {
    try {
        $pinfo = New-Object System.Diagnostics.ProcessStartInfo
        $pinfo.FileName = $ExePath
        $pinfo.Arguments = "--version"
        $pinfo.UseShellExecute = $false
        $pinfo.RedirectStandardOutput = $true
        $pinfo.RedirectStandardError = $true
        $pinfo.CreateNoWindow = $true
        $p = [System.Diagnostics.Process]::Start($pinfo)
        $output = $p.StandardOutput.ReadToEnd()
        $null = $p.StandardError.ReadToEnd()
        [void]$p.WaitForExit(3000)
        if ($output -match "NetTrigger\s+([0-9]+\.[0-9]+\.[0-9]+)") {
            $localVerStr = $matches[1]
            $localVer = [System.Version]$localVerStr
        }
    } catch {}

    # 若未通过命令行获取，尝试读取文件属性
    if (-not $localVer) {
        try {
            $fvi = (Get-Item -LiteralPath $ExePath).VersionInfo
            if ($fvi.ProductVersion -match "([0-9]+\.[0-9]+\.[0-9]+)") {
                $localVerStr = $matches[1]
                $localVer = [System.Version]$localVerStr
            }
        } catch {}
    }
}

if ($localVerStr) {
    Write-Color "[本地环境] " -ForegroundColor Green -NoNewline
    Write-Host "已检测到当前安装版本: " -NoNewline
    Write-Color "v$localVerStr" -ForegroundColor Yellow -NoNewline
    Write-Host " ($ExePath)"
} else {
    Write-Color "[本地环境] " -ForegroundColor Yellow -NoNewline
    Write-Host "当前目录下未检测到 NetTrigger.exe (将仅查询官方最新发布版本)"
}

Write-Host ""
Write-Color "正在连接 GitHub 检查最新发布版本，请稍候..." -ForegroundColor Gray

# 2. 查询 GitHub Releases 最新版本
$repoOwner = "Sacilave"
$repoName = "net-trigger"
$apiUrl = "https://api.github.com/repos/$repoOwner/$repoName/releases/latest"
$releasePageUrl = "https://github.com/$repoOwner/$repoName/releases/latest"

$latestRelease = $null
$queryError = $null

try {
    $headers = @{
        "User-Agent" = "NetTrigger-Update-Checker"
        "Accept"     = "application/vnd.github.v3+json"
    }
    $latestRelease = Invoke-RestMethod -Uri $apiUrl -Headers $headers -TimeoutSec 12 -ErrorAction Stop
} catch {
    $queryError = $_.Exception.Message
}

if (-not $latestRelease) {
    Write-Host ""
    Write-Color "[提示] 暂时无法直连 GitHub API 获取更新信息" -ForegroundColor Yellow
    Write-Color "详细原因: $queryError" -ForegroundColor DarkGray
    Write-Host ""
    Write-Color "可能原因：" -ForegroundColor Yellow
    Write-Host "  1. 当前校园网尚未登录认证，或处于离线断网状态"
    Write-Host "  2. 所在网络访问 api.github.com 受到限制或超时"
    Write-Host ""
    $openWeb = Read-Host "是否在浏览器中直接打开 GitHub Release 发布页面查看？(Y/N, 默认 Y)"
    if ($openWeb -ne "N" -and $openWeb -ne "n") {
        Start-Process $releasePageUrl
    }
    return
}

# 提取最新版本号
$remoteTag = $latestRelease.tag_name
$remoteVerStr = $remoteTag.TrimStart('v').Trim()
$remoteVer = $null
try {
    if ($remoteVerStr -match "^([0-9]+\.[0-9]+\.[0-9]+)") {
        $remoteVer = [System.Version]$matches[1]
    }
} catch {}

$publishTime = ""
if ($latestRelease.published_at) {
    try {
        $dt = [DateTime]::Parse($latestRelease.published_at).ToLocalTime()
        $publishTime = $dt.ToString("yyyy-MM-dd HH:mm:ss")
    } catch {
        $publishTime = $latestRelease.published_at
    }
}

Write-Host ""
Write-Color "----------------- 版本检测结果 -----------------" -ForegroundColor Cyan
Write-Color "最新发布版本: " -ForegroundColor White -NoNewline
Write-Color "v$remoteVerStr" -ForegroundColor Green -NoNewline
if ($latestRelease.name) {
    Write-Host " ($($latestRelease.name))"
} else {
    Write-Host ""
}

if ($publishTime) {
    Write-Color "官方发布时间: " -ForegroundColor Gray -NoNewline
    Write-Host $publishTime
}

# 比对版本
$hasNewVersion = $false
if ($localVer -and $remoteVer) {
    if ($remoteVer -gt $localVer) {
        $hasNewVersion = $true
    }
} elseif (-not $localVer -and $remoteVer) {
    $hasNewVersion = $true
}

Write-Host ""
if (-not $hasNewVersion -and $localVer) {
    Write-Color "🎉 恭喜！当前已是最新版本 (v$localVerStr)，无需更新。" -ForegroundColor Green
    Write-Host ""
    Write-Color "当前版本运行良好，未发现更新。" -ForegroundColor Gray
    Write-Host ""
    $recheck = Read-Host "是否在浏览器中打开 GitHub 发布主页？(Y/N, 默认 N)"
    if ($recheck -eq "Y" -or $recheck -eq "y") {
        Start-Process $releasePageUrl
    }
    return
}

# 发现新版本！
Write-Color "⚡ 发现新版本可用！" -ForegroundColor Yellow
if ($localVerStr) {
    Write-Color "   当前本地版本: v$localVerStr  -->  最新发布版本: v$remoteVerStr" -ForegroundColor Yellow
} else {
    Write-Color "   最新可用版本: v$remoteVerStr" -ForegroundColor Yellow
}

# 显示更新日志简要
if ($latestRelease.body) {
    Write-Host ""
    Write-Color "【更新摘要】" -ForegroundColor Cyan
    $bodyLines = $latestRelease.body.Split("`n") | Select-Object -First 10
    foreach ($line in $bodyLines) {
        Write-Host "  $($line.Trim())" -ForegroundColor Gray
    }
}

Write-Host ""
Write-Color "----------------- 升级选项 -----------------" -ForegroundColor Cyan
Write-Host "  [1] 一键自动下载并热替换当前便携版 (NetTrigger.exe) ⭐ 推荐"
Write-Host "  [2] 下载一键安装向导包 (NetTrigger-v$remoteVerStr-Setup.exe)"
Write-Host "  [3] 在默认浏览器中打开 GitHub Release 发布主页"
Write-Host "  [0] 暂不更新，退出"
Write-Host ""

$action = Read-Host "请输入操作选项 [默认 1]"
if ([string]::IsNullOrWhiteSpace($action)) { $action = "1" }

switch ($action) {
    "1" {
        # 自动下载便携单文件 x64 版
        $asset = $latestRelease.assets | Where-Object { $_.name -like "*windows-x64.exe" } | Select-Object -First 1
        if (-not $asset) {
            $asset = $latestRelease.assets | Where-Object { $_.name -like "*.exe" -and $_.name -notlike "*Setup*" } | Select-Object -First 1
        }

        if (-not $asset) {
            Write-Color "[错误] 未能找到便携版 exe 下载资源，正在为您打开网页发布页..." -ForegroundColor Red
            Start-Process $releasePageUrl
            return
        }

        $downloadUrl = $asset.browser_download_url
        $tempExe = Join-Path $ScriptDir "NetTrigger.exe.download"
        $targetExe = Join-Path $ScriptDir "NetTrigger.exe"
        $backupExe = Join-Path $ScriptDir "NetTrigger.exe.bak"

        Write-Host ""
        Write-Color "准备下载便携版: $($asset.name)" -ForegroundColor Cyan
        Write-Color "下载直链: $downloadUrl" -ForegroundColor DarkGray
        Write-Host ""
        Write-Color "正在下载新版本，请稍候..." -ForegroundColor Yellow

        try {
            $webClient = New-Object System.Net.WebClient
            $webClient.Headers.Add("User-Agent", "NetTrigger-Update-Checker")
            $webClient.DownloadFile($downloadUrl, $tempExe)
        } catch {
            Write-Color "[错误] 下载失败: $($_.Exception.Message)" -ForegroundColor Red
            Write-Host "建议直接打开浏览器下载："
            Start-Process $releasePageUrl
            return
        }

        if (-not (Test-Path -LiteralPath $tempExe) -or (Get-Item -LiteralPath $tempExe).Length -lt 100000) {
            Write-Color "[错误] 下载文件异常或不完整！" -ForegroundColor Red
            if (Test-Path -LiteralPath $tempExe) { Remove-Item -LiteralPath $tempExe -Force }
            return
        }

        # 检查是否正在运行，先优雅关闭旧版
        $runningProc = Get-Process -Name "NetTrigger" -ErrorAction SilentlyContinue
        if ($runningProc) {
            Write-Color "检测到 NetTrigger 正在后台运行，正在自动退出旧进程..." -ForegroundColor Yellow
            try {
                Stop-Process -Name "NetTrigger" -Force -ErrorAction SilentlyContinue
                Start-Sleep -Milliseconds 600
            } catch {}
        }

        # 备份旧版并替换
        if (Test-Path -LiteralPath $targetExe) {
            try {
                Move-Item -LiteralPath $targetExe -Destination $backupExe -Force
            } catch {
                Write-Color "[警告] 无法重命名旧版文件，可能进程尚未完全释放。" -ForegroundColor Red
            }
        }

        try {
            Move-Item -LiteralPath $tempExe -Destination $targetExe -Force
            Write-Host ""
            Write-Color "=================================================================" -ForegroundColor Green
            Write-Color "🎉 恭喜！NetTrigger 已成功升级到最新版本: v$remoteVerStr" -ForegroundColor Green
            Write-Color "=================================================================" -ForegroundColor Green
            Write-Host ""

            $startNow = Read-Host "是否立即启动新版 NetTrigger？(Y/N, 默认 Y)"
            if ($startNow -ne "N" -and $startNow -ne "n") {
                Start-Process -FilePath $targetExe
                Write-Color "⚡ NetTrigger 已在后台启动！已恢复网络守护状态。" -ForegroundColor Cyan
            }
        } catch {
            Write-Color "[错误] 替换新版本失败: $($_.Exception.Message)" -ForegroundColor Red
            if (Test-Path -LiteralPath $backupExe) {
                Write-Color "正在恢复备份..." -ForegroundColor Yellow
                Move-Item -LiteralPath $backupExe -Destination $targetExe -Force -ErrorAction SilentlyContinue
            }
        }
    }

    "2" {
        # 下载安装向导 Setup.exe
        $setupAsset = $latestRelease.assets | Where-Object { $_.name -like "*Setup.exe" } | Select-Object -First 1
        if (-not $setupAsset) {
            Write-Color "[提示] 远程发布包中未找到独立的 Setup.exe，为您打开 Release 页面..." -ForegroundColor Yellow
            Start-Process $releasePageUrl
            return
        }

        $setupUrl = $setupAsset.browser_download_url
        $setupPath = Join-Path $ScriptDir $setupAsset.name

        Write-Host ""
        Write-Color "正在下载安装包: $($setupAsset.name)" -ForegroundColor Cyan
        try {
            $webClient = New-Object System.Net.WebClient
            $webClient.Headers.Add("User-Agent", "NetTrigger-Update-Checker")
            $webClient.DownloadFile($setupUrl, $setupPath)
            Write-Color "安装向导下载完成: $setupPath" -ForegroundColor Green
            Write-Host ""
            $runSetup = Read-Host "是否立即运行安装向导？(Y/N, 默认 Y)"
            if ($runSetup -ne "N" -and $runSetup -ne "n") {
                Start-Process -FilePath $setupPath
            }
        } catch {
            Write-Color "[错误] 下载安装向导失败: $($_.Exception.Message)" -ForegroundColor Red
            Start-Process $releasePageUrl
        }
    }

    "3" {
        Write-Color "正在打开 GitHub Release 官方发布页面..." -ForegroundColor Cyan
        Start-Process $releasePageUrl
    }

    default {
        Write-Host "已取消操作。"
    }
}
