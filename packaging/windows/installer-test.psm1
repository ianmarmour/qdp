# What the installer tests share: running msiexec, and failing with a reason.

# Runs msiexec quietly with ARGUMENTS, logging to LOG, and fails unless it
# succeeds.
function Invoke-Installer([string[]] $Arguments, [string] $Log) {
    $process = Start-Process msiexec.exe -Wait -PassThru `
        -ArgumentList ($Arguments + @('/qn', '/l*v', $Log))
    if ($process.ExitCode -ne 0) {
        throw "msiexec $($Arguments -join ' ') failed with $($process.ExitCode); see $Log"
    }
}

# Fails with MESSAGE unless CONDITION holds.
function Assert-That([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

# Returns whether the system's PATH has FOLDER.
function Test-OnPath([string] $Folder) {
    $path = [Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';'
    return $path -contains $Folder -or $path -contains "$Folder\"
}

Export-ModuleMember -Function Invoke-Installer, Assert-That, Test-OnPath
