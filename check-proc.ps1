$procs = Get-Process | Where-Object { $_.ProcessName -like '*win-top*' -or $_.ProcessName -like '*Win_Top*' }
if ($procs) {
    $procs | Format-Table Id, ProcessName, MainWindowTitle -AutoSize
} else {
    $allProcs = Get-Process | Where-Object { $_.MainWindowTitle -ne '' } | Select-Object Id, ProcessName, MainWindowTitle
    $allProcs | Format-Table -AutoSize
}
