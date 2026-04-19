Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap(32, 32)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(124, 58, 237))
$g.FillRectangle($brush, 0, 0, 32, 32)
$brush.Dispose()
$g.Dispose()
$pngPath = 'F:\Code\Win-Top\src-tauri\icons\icon.png'
$bmp.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)

$pngBmp = New-Object System.Drawing.Bitmap($pngPath)
$icoPath = 'F:\Code\Win-Top\src-tauri\icons\icon.ico'
$icon = [System.Drawing.Icon]::FromHandle($pngBmp.GetHicon())
$stream = [System.IO.File]::Create($icoPath)
$icon.Save($stream)
$stream.Close()
$icon.Dispose()
$pngBmp.Dispose()
$bmp.Dispose()
Write-Output 'Icon created'
