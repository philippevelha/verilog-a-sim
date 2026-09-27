# =============================================================================================
# verilog-a-sim — platform benchmark, WINDOWS (x86-64), for Windows PowerShell 5.1 or later
# =============================================================================================
#
# What it does: runs a fixed set of decks with the compiled `va-cli.exe` from a release archive,
# times each one, and writes `benchmark.txt` holding
#   - the machine's specifications (CPU, cores, memory, Windows version, power source and power
#     plan), the baseline for comparing platforms;
#   - the wall time and exit code of every deck;
#   - a SHA-256 of each deck's output. Since 2.0.0 the output is meant to be bit-identical on
#     Linux, macOS and Windows, so equal hashes across the three benchmark.txt files confirm it.
#
# How to run it:
#   1. Download `va-cli-v2.0.0-x86_64-pc-windows-msvc.zip` from
#      https://github.com/philippevelha/verilog-a-sim/releases, then in PowerShell:
#          cd $HOME\Downloads
#          Expand-Archive va-cli-v2.0.0-x86_64-pc-windows-msvc.zip -DestinationPath .
#          cd va-cli-v2.0.0-x86_64-pc-windows-msvc
#          Get-ChildItem -Recurse | Unblock-File
#      The last line clears the "downloaded from the internet" mark, which can otherwise make
#      Windows block or prompt for the unsigned executable.
#   2. For a clean baseline: plug in the charger, choose the "Best performance" / "High
#      performance" power mode, close other heavy programs, and leave the machine alone while
#      it runs.
#   3. Copy everything from the `$Long = ...` line down to the end and paste it into the
#      PowerShell window, inside that directory. Pasting works whatever the script execution
#      policy is. It takes about 4 minutes. To also run the two long decks (c432's 1 ns transient,
#      ~15 min; c7552's .op, ~6 min and ~4 GB of RAM), change $false to $true before pasting.
#   4. Send back `benchmark.txt` (it is written in the same directory). Per-deck outputs are
#      kept in `bench-out\` in case a hash differs and the outputs need comparing.
#
# Timing notes: each figure is one run's wall time, and includes compiling the Verilog-A model
# (tens of ms) and writing the output. The output is copied byte for byte from the program's
# stdout stream; PowerShell's own `>` would re-encode it and change the hashes.
# Decks run with the default thread count (one per logical CPU), plus c432's .op once with
# `--threads 1`, to show how much the parallel evaluation buys.
# =============================================================================================

$Long = $false
& {
  $ErrorActionPreference = 'Stop'
  $va = Join-Path $PWD 'va-cli.exe'
  $out = Join-Path $PWD 'benchmark.txt'
  if (-not ((Test-Path $va) -and (Test-Path 'benchmark') -and (Test-Path 'circuits'))) {
    Write-Host 'Run this inside the unpacked va-cli archive directory (va-cli.exe, circuits\, benchmark\).'
    return
  }
  New-Item -ItemType Directory -Force 'bench-out' | Out-Null
  $psp = 'benchmark/models/psp103/psp103.va'

  $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
  $cs = Get-CimInstance Win32_ComputerSystem
  $os = Get-CimInstance Win32_OperatingSystem
  $bat = Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $bat) { $power = 'mains (no battery: desktop or VM)' }
  elseif ($bat.BatteryStatus -eq 2) { $power = 'mains (laptop, charger connected)' }
  else { $power = 'BATTERY (timings not a baseline)' }
  $plan = (powercfg /getactivescheme) -replace '^.*\((.*)\).*$', '$1'
  $cpuLoad = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average

  $lines = @(
    'verilog-a-sim platform benchmark - Windows'
    ('date (UTC):   ' + (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd HH:mm:ss'))
    ('archive dir:  ' + (Split-Path -Leaf $PWD))
    ('va-cli sha256: ' + (Get-FileHash -Algorithm SHA256 $va).Hash.ToLower())
    ''
    '== machine'
    ('os:           ' + $os.Caption + ' ' + $os.Version + ' (build ' + $os.BuildNumber + ')')
    ('cpu:          ' + $cpu.Name.Trim())
    ('cores:        ' + $cpu.NumberOfCores)
    ('logical cpus: ' + $cpu.NumberOfLogicalProcessors)
    ('max MHz:      ' + $cpu.MaxClockSpeed)
    ('memory:       ' + [math]::Round($cs.TotalPhysicalMemory / 1GB, 1) + ' GiB')
    ('power:        ' + $power)
    ('power plan:   ' + $plan)
    ('cpu load:     ' + $cpuLoad + ' % (before the run)')
    ''
    '== decks (wall time, one run each)'
    ('{0,-48} {1,8} {2,5}  {3}' -f 'deck', 'seconds', 'exit', 'sha256(stdout)')
  )
  $lines | Set-Content -Encoding UTF8 $out

  function Run-Deck([string]$label, [string[]]$vaArgs) {
    $name = ($label -replace '[/ ]', '_')
    $f = Join-Path $PWD ("bench-out\" + $name + '.out')
    # Launched through System.Diagnostics.Process, not Start-Process: measured on the same
    # machine, c432's .op took 15.3 s under Start-Process against 9.2 s this way (and 10.7 s from
    # bash), so Start-Process would have benchmarked itself. Stdout is copied from the raw byte
    # stream, so the file (and its hash) is exactly what va-cli wrote; stderr is drained in the
    # background so neither pipe can fill and stall the run.
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $va
    $psi.Arguments = $vaArgs -join ' '
    $psi.WorkingDirectory = $PWD.Path
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = [Diagnostics.Process]::Start($psi)
    $p.StandardInput.Close()
    $errText = $p.StandardError.ReadToEndAsync()
    $fs = [IO.File]::Create($f)
    $p.StandardOutput.BaseStream.CopyTo($fs)
    $fs.Close()
    $p.WaitForExit()
    $sw.Stop()
    [IO.File]::WriteAllText($f + '.err', $errText.Result)
    # Invariant culture: a decimal point, not the locale's comma, so the figures compare
    # directly with the Linux and macOS files.
    $secs = $sw.Elapsed.TotalSeconds.ToString('F2', [Globalization.CultureInfo]::InvariantCulture)
    $hash = (Get-FileHash -Algorithm SHA256 $f).Hash.ToLower()
    ('{0,-48} {1,8} {2,5}  {3}' -f $label, $secs, $p.ExitCode, $hash) | Add-Content -Encoding UTF8 $out
    Write-Host ("  " + $label + ': ' + $secs + ' s (exit ' + $p.ExitCode + ')')
  }

  Write-Host "Running the benchmark; results go to $out"
  Run-Deck 'rectifier (.tran, diode)'         @('sim', 'circuits/rectifier.net', '--model', 'models/diode.va', '--tran')
  Run-Deck 'ring_osc (.tran, reference BJTs)' @('sim', 'circuits/ring_osc.net', '--tran')
  Run-Deck 'nand/psp103 (.op)'                @('sim', 'circuits/benchmark/nand/psp103.net', '--model', $psp, '--report', 'out')
  Run-Deck 'psp103_inverter_tran (.tran)'     @('sim', 'circuits/benchmark/psp103_inverter_tran.net', '--model', $psp, '--tran', '--report', 'in,out')
  Run-Deck 'iscas85/c17 (.op)'                @('sim', 'circuits/benchmark/iscas85/c17.net', '--model', $psp)
  Run-Deck 'iscas85/c432 (.op)'               @('sim', 'circuits/benchmark/iscas85/c432.net', '--model', $psp)
  Run-Deck 'iscas85/c432 (.op, --threads 1)'  @('sim', 'circuits/benchmark/iscas85/c432.net', '--model', $psp, '--threads', '1')
  Run-Deck 'iscas85/c499 (.op)'               @('sim', 'circuits/benchmark/iscas85/c499.net', '--model', $psp)
  Run-Deck 'iscas85/c880 (.op)'               @('sim', 'circuits/benchmark/iscas85/c880.net', '--model', $psp)
  Run-Deck 'iscas85/c17_tran (.tran 12 ns)'   @('sim', 'circuits/benchmark/iscas85/c17_tran.net', '--model', $psp, '--tran')
  if ($Long) {
    Run-Deck 'iscas85/c432_tran (.tran 1 ns)' @('sim', 'circuits/benchmark/iscas85/c432_tran.net', '--model', $psp, '--tran')
    Run-Deck 'iscas85/c7552 (.op)'            @('sim', 'circuits/benchmark/iscas85/c7552.net', '--model', $psp)
  }
  ('== end ' + (Get-Date).ToUniversalTime().ToString('HH:mm:ss') + ' UTC') | Add-Content -Encoding UTF8 $out
  Write-Host ''
  Get-Content $out | Write-Host
  Write-Host ''
  Write-Host "Done. Send back: $out"
}
