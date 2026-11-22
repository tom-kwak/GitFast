# Installs (or updates) gf from the latest GitHub release, for the current
# user (no administrator rights needed):
#
#   irm https://raw.githubusercontent.com/tom-kwak/GitFast/main/scripts/install.ps1 | iex
#
# Goes to %LOCALAPPDATA%\Programs\gf, adds that folder to the user PATH and
# a Start menu shortcut. GF_ZIP_URL / GF_INSTALL_DIR override where from
# and where to (for testing a build), as in install.sh.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

& {
    $repo = 'tom-kwak/GitFast'
    $name = 'gf-windows-x64'
    $url = if ($env:GF_ZIP_URL) { $env:GF_ZIP_URL } else {
        "https://github.com/$repo/releases/latest/download/$name.zip"
    }
    $dest = if ($env:GF_INSTALL_DIR) { $env:GF_INSTALL_DIR } else {
        Join-Path $env:LOCALAPPDATA 'Programs\gf'
    }

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("gf-install-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Force $tmp | Out-Null
    try {
        Write-Host 'Downloading the latest gf'
        $zip = Join-Path $tmp "$name.zip"
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip
        # Checked when the release has a checksum next to it (local builds
        # given with GF_ZIP_URL as a path may not).
        try {
            $expected = (Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256").Content
            if ($expected -is [byte[]]) { $expected = [System.Text.Encoding]::ASCII.GetString($expected) }
            $expected = ($expected -split '\s+')[0].ToLowerInvariant()
        } catch {
            $expected = $null
            Write-Warning 'no checksum found next to the download: not verified'
        }
        $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
        if ($expected -and $expected -ne $actual) {
            throw "checksum mismatch: expected $expected, got $actual"
        }
        # Downloaded files carry the web's mark: clear it so SmartScreen
        # does not block every start.
        Unblock-File $zip

        if (Get-Process -Name gf -ErrorAction SilentlyContinue) {
            Write-Warning 'gf is running: quit it, then run this again to update.'
            return
        }
        New-Item -ItemType Directory -Force $dest | Out-Null
        Expand-Archive -Path $zip -DestinationPath $dest -Force
        Get-ChildItem $dest -Recurse | Unblock-File

        # User PATH, so `gf` works from a terminal.
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $parts = @($userPath -split ';' | Where-Object { $_ })
        if ($parts -notcontains $dest) {
            [Environment]::SetEnvironmentVariable('Path', (($parts + $dest) -join ';'), 'User')
            Write-Host "Added $dest to your PATH (new terminals see it)."
        }

        # Start menu shortcut.
        $programs = [Environment]::GetFolderPath('Programs')
        $shell = New-Object -ComObject WScript.Shell
        $link = $shell.CreateShortcut((Join-Path $programs 'gf.lnk'))
        $link.TargetPath = Join-Path $dest 'gf.exe'
        $link.WorkingDirectory = $dest
        $link.Save()

        if (-not (Get-Command git -ErrorAction SilentlyContinue) -and
            -not (Test-Path (Join-Path $env:ProgramFiles 'Git\cmd\git.exe'))) {
            Write-Warning 'git was not found. gf needs Git for Windows: https://git-scm.com/download/win'
        }
        Write-Host "Installed $(Join-Path $dest 'gf.exe')"
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
