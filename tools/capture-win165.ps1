param(
  [string]$FixtureRoot = (Split-Path (Split-Path $PSScriptRoot)),
  [string]$Output = (Join-Path (Split-Path $PSScriptRoot) 'outputs/win165'),
  [ValidateRange(1,120)][int]$Frames = 24,
  [ValidateRange(0,3)][int[]]$Slots = (0..3),
  [string]$SessionFrom,
  [ValidateSet('dino','ssf2','kof97')][string]$Game='dino',
  [string]$SaveRoot,
  [switch]$TraceSoundSlices,
  [switch]$TraceNeoMix,
  [switch]$TraceNeoSources,
  [string]$Debugger = 'C:\Program Files (x86)\Windows Kits\10\Debuggers\x86\cdb.exe'
)
$ErrorActionPreference='Stop'
$FixtureRoot=(Resolve-Path -LiteralPath $FixtureRoot).Path
if($SessionFrom) { $SessionFrom=(Resolve-Path -LiteralPath $SessionFrom).Path }
if(!$SaveRoot){$SaveRoot=Join-Path $FixtureRoot 'saves'}
$SaveRoot=(Resolve-Path -LiteralPath $SaveRoot).Path
$platform=switch($Game){'ssf2'{'cps2'} 'kof97'{'neogeo'} default{'cps1'}}
$frameEntry=switch($platform){'cps2'{'421eb0'} 'neogeo'{'42c9f0'} default{'4219d0'}}
$saveReturn=switch($platform){'cps2'{'45e178'} 'neogeo'{'45e2d8'} default{'45e1fc'}}
if(($TraceNeoMix -or $TraceNeoSources) -and $platform -ne 'neogeo'){throw 'The traced mixer addresses are for Neo Geo.'}
if($TraceSoundSlices -and $platform -ne 'cps1'){throw 'The traced slice addresses are for CPS1.'}
$original=Join-Path $FixtureRoot 'WinKawaks.exe'
$stream=[IO.File]::OpenRead($original)
$sha=[Security.Cryptography.SHA256]::Create()
try { $hash=[BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-','') }
finally { $sha.Dispose(); $stream.Dispose() }
if($hash -ne 'a85105f3e45b737b4b02e4467bc9a7628ce04592b4e3a275d1576443cc2708a9') {
  throw 'Unexpected WinKawaks executable. The debugger addresses require the verified 1.65 build.'
}
if(!(Test-Path -LiteralPath $Debugger -PathType Leaf)) { throw 'Specify the Windows SDK x86 cdb.exe with -Debugger.' }
if(Test-Path -LiteralPath $Output) { throw 'Pass an unused directory to -Output.' }
$work=(New-Item -Path $Output -ItemType Directory).FullName
foreach($dir in @('ini','eeprom','lang','saves','roms/cps1','roms/cps2','roms/neogeo','capture','sshots','recinput','blend','cheats','tracklst')) {
  New-Item -Path (Join-Path $work $dir) -ItemType Directory -Force | Out-Null
}
$required=@('WinKawaks.exe',"roms/$platform/$Game.zip")
foreach($slot in $Slots){
  Copy-Item -LiteralPath (Join-Path $SaveRoot "$Game.ks$slot") -Destination (Join-Path $work "saves/$Game.ks$slot")
}
$optional=@('WinKawaks.rom','Skin.bmp','defaultkeysCPS.ini','lang/English.lng','ini/dino.ini','eeprom/dino.epm')
if($platform -eq 'neogeo'){$required += 'roms/neogeo/neogeo.zip'}
foreach($file in $required+$optional) {
  $from=Join-Path $FixtureRoot $file
  if($file -in $required -or (Test-Path -LiteralPath $from)) {
    Copy-Item -LiteralPath $from -Destination (Join-Path $work $file)
  }
}
$ini=@'
[Misc]
LanguagePack=English
RunCount=2
AutoStartGame=1
EnableCPS1=1
EnableCPS2=1
EnableMVS=1
AutoPause=0
SoundFreq=22050
MonoSound=0
ReverseStereo=0
LowPassFilter=0
QSoundAmplification=0
TaskPriorityLevel=32
[VideoSettings]
FrameSkip=0
AutoFrameSkip=0
BlendFrames=0
EnableTransparencies=0
TrueGammaRed=256
TrueGammaGreen=256
TrueGammaBlue=256
[Path]
RomPath1=.\roms\cps2
RomPath2=.\roms\cps1
RomPath3=.\roms\neogeo
RomPath4=.\roms
RomPath5=.\roms
SavesPath=.\saves
EEPROMPath=.\eeprom
INIPath=.\ini
CapturePath=.\capture
LanguagePath=.\lang
'@
$ini | Set-Content -LiteralPath (Join-Path $work 'WinKawaks.ini') -Encoding Ascii
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win165 {
  [DllImport("user32")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@
$exe=Join-Path $work 'WinKawaks.exe'
foreach($slot in $Slots) {
  $dir=Join-Path $work "slot$slot"
  New-Item $dir -Type Directory -Force | Out-Null
  $script="slot$slot\debug.txt"
  $log="slot$slot\debug-$PID.log"
  $commands=[Collections.Generic.List[string]]::new()
  $commands.Add(('bp /1 {0} ".echo GAME_ACTIVE; gc"' -f $frameEntry))
  $commands.Add("bp $saveReturn")
  $commands.Add('g')
  $commands.Add(('.if (@eip != {0}) {{ q }}' -f $saveReturn))
  $commands.Add('bc *')
  $commands.Add('.echo KWK4_RESTORED')
  if($SessionFrom) {
    $session=Join-Path $SessionFrom "slot$slot"
    $phase=[BitConverter]::ToUInt32([IO.File]::ReadAllBytes((Join-Path $session 'pre.phase')),0)
    $latch=[BitConverter]::ToUInt32([IO.File]::ReadAllBytes((Join-Path $session 'pre.latch')),0)
    $commands.Add(('ed 7d54f0 {0:x}' -f $phase))
    $commands.Add(('ed 7ff270 {0:x}' -f $latch))
    # This carry is absent from KWK4 and was not captured by older scripts.
    $carryPath=Join-Path $session 'pre.irq-cycles'
    $carry=if(Test-Path -LiteralPath $carryPath) { [BitConverter]::ToUInt32([IO.File]::ReadAllBytes($carryPath),0) } else {0}
    $commands.Add(('ed 7b04d0 {0:x}' -f $carry))
  }
  $commands.Add('dd 75e050 L1')
  $commands.Add('dd 7c6f10 L1')
  $commands.Add('dd 7ea7e0 L3')
  $commands.Add('dd 7d54ec L2')
  $commands.Add('dd 7ff1a8 L3')
  if($platform -eq 'neogeo') {
    $commands.Add(".writemem slot$slot/pre.animation 7dd698 L4")
  }
  if($platform -ne 'neogeo') {
    $commands.Add(".writemem slot$slot/pre.voices 7c6fa0 L300")
  $commands.Add(".writemem slot$slot/pre.phase 7d54f0 L4")
  $commands.Add(".writemem slot$slot/pre.irq-cycles 7b04d0 L4")
  $commands.Add(".writemem slot$slot/pre.latch 7ff270 L4")
  $commands.Add('bp9 4092c0 ".printf \"QS %02x %04x\n\", poi(@esp+4), poi(@esp+8); gc"')
  }
  if($TraceSoundSlices) {
    # Before/after the scheduler checks the periodic threshold, then actual vector entries.
    $commands.Add('bp1 421ae0 ".printf \"SL %x %x %x %x \", @edi, wo(7b04bc), @eax, @esi; gc"')
    $commands.Add('bp2 421b23 ".printf \"END %x %x %x \", wo(7b04bc), poi(7d54f0), by(7b04ce); gc"')
    $commands.Add('bp3 49be98 ".printf \"IRQ I %x %x \", wo(7b04bc), @edi; gc"')
    $commands.Add('bp4 49c0df ".printf \"IRQ D %x %x \", (@esi-@ebp)&ffff, poi(7b04d8)-@edi; gc"')
  }
  if($TraceNeoMix) {
    $commands.Add('bp9 403dd0 ".printf \"YMW %x %x\n\", poi(@esp+8), poi(@esp+c); gc"')
    # Cumulative unquantized pan buses for every sample in the capture.
    # D is after Delta-T, F follows an FM channel, A follows an ADPCM-A voice.
    $commands.Add('bp1 402355 ".printf \"D %x %x %x\n\", poi(7b8184), poi(7b8180), poi(7b8188); gc"')
    $commands.Add('bp2 402363 ".printf \"F %x %x %x %x\n\", (@ebp-7b8194)/4, poi(7b8184), poi(7b8180), poi(7b8188); gc"')
    $commands.Add('bp3 402396 ".printf \"A %x %x %x %x\n\", 6-@ebx, poi(7b8184), poi(7b8180), poi(7b8188); gc"')
    $commands.Add('bp4 402418 ".printf \"A %x %x %x %x\n\", 6-poi(@esp+20), poi(7b8184), poi(7b8180), poi(7b8188); gc"')
  }
  if($TraceNeoSources) {
    if(!$TraceNeoMix) {
      $commands.Add('bp5 402355 ".printf \"DT %x %x %x\n\", poi(7b8184), poi(7b8180), poi(7b8188); gc"')
    }
    $commands.Add('bp6 402371 ".printf \"FM %x %x %x\n\", poi(7b8184), poi(7b8180), poi(7b8188); gc"')
    $commands.Add('bp7 441a03 ".printf \"AY %x %x %x\n\", wo(poi(7c7754)+@ecx*2), wo(poi(7c7758)+@ecx*2), wo(poi(7c775c)+@ecx*2); gc"')
  }
  for($frame=1;$frame -le $Frames;$frame++) {
    $file="slot$slot/f{0:000}" -f $frame
    if($platform -eq 'neogeo'){
      $commands.Add('bp0 409cf4')
      $commands.Add('g')
      $commands.Add('.if (@eip != 409cf4) { q }')
      $commands.Add(".echo MIX_FRAME $frame")
    } else {
    $commands.Add('bp0 409310')
    $commands.Add('g')
    $commands.Add('.if (@eip != 409310) { q }')
    $commands.Add(".echo MIX_FRAME $frame")
    $commands.Add(".writemem $file.mix-in 7c6fa0 L300")
    $commands.Add(".writemem $file.phase 7d54f0 L4")
    $commands.Add(".writemem $file.z80 7b046c L64")
    $commands.Add('bc 0')
    $commands.Add('bp0 4094a0')
    $commands.Add('g')
    $commands.Add('.if (@eip != 4094a0) { q }')
    }
    if($TraceNeoSources) {
      $commands.Add(".writemem $file.fm-state poi(7c61d4) L3420")
      $commands.Add(".writemem $file.psg-state 7b3420 Lf8")
    }
    $commands.Add(".writemem $file.left poi(7c774c) L2de")
    $commands.Add(".writemem $file.right poi(7c7750) L2de")
    if($platform -ne 'neogeo'){$commands.Add(".writemem $file.voices 7c6fa0 L300")}
    $commands.Add('bc 0')
    $commands.Add('bp0 443741')
    $commands.Add('g')
    $commands.Add('.if (@eip != 443741) { q }')
    $commands.Add(".writemem $file.rgb16 (poi(840ef8)+7040) L31000")
    if($platform -eq 'neogeo') {
      $commands.Add(".writemem $file.vram poi(885770) L20000")
      $commands.Add(".writemem $file.palette poi(8857a8) L4000")
      $commands.Add(".writemem $file.animation 7dd6c8 L2")
    } else {
      $commands.Add(".writemem $file.vram poi(885770) L30000")
      $commands.Add(".writemem $file.regs poi(88576c) L200")
    }
    if($platform -eq 'cps2') {
      $commands.Add(".writemem $file.objects0 poi(885788) L8000")
      $commands.Add(".writemem $file.objects1 poi(885798) L8000")
    } elseif($platform -eq 'cps1') {
      $commands.Add(".writemem $file.objects poi(7dd668) L800")
    }
    $commands.Add('bc 0')
  }
  $commands.Add('.echo CAPTURE_COMPLETE')
  $commands.Add('q')
  Set-Content (Join-Path $work $script) $commands
  $opts='-y . -logo {0} -cf {1} "{2}" {3}' -f $log,$script,$exe,$Game
  $debug=Start-Process -FilePath $Debugger -ArgumentList $opts -WorkingDirectory $work -WindowStyle Hidden -PassThru
  try {
    $path=Join-Path $work $log
    $end=[DateTime]::UtcNow.AddSeconds(15)
    while([DateTime]::UtcNow -lt $end) {
      if((Test-Path $path) -and (Select-String -Path $path -Pattern '^GAME_ACTIVE$' -Quiet)) {break}
      Start-Sleep -Milliseconds 150
    }
    $app=Get-CimInstance Win32_Process -Filter "ParentProcessId = $($debug.Id) AND Name = 'WinKawaks.exe'"
    if(!$app) {throw "Slot $slot debugger did not start WinKawaks"}
    $hwnd=(Get-Process -Id $app.ProcessId).MainWindowHandle
    if(!$hwnd) {throw "Slot $slot cannot locate the isolated WinKawaks window"}
    if(-not [Win165]::PostMessage($hwnd,0x111,[IntPtr](32827+$slot),[IntPtr]::Zero)) {throw 'Failed to select slot'}
    Start-Sleep -Milliseconds 150
    if(-not [Win165]::PostMessage($hwnd,0x111,[IntPtr]32825,[IntPtr]::Zero)) {throw 'Failed to request state load'}
    $timeout=if($TraceNeoMix -or $TraceNeoSources){20000+6000*$Frames}else{20000+1000*$Frames}
    if(!$debug.WaitForExit($timeout)) {throw "Slot $slot capture timed out. Inspect $path"}
    if((Select-String -Path $path -Pattern 'Unable to insert breakpoint|Unable to write' -Quiet) -or !(Select-String -Path $path -Pattern '^CAPTURE_COMPLETE$' -Quiet)) {throw "Slot $slot incomplete. Inspect $path"}
    Copy-Item $path (Join-Path $dir "capture.log") -Force
    Write-Output "Slot $slot captured $Frames reference frames."
  } finally {
    if(!$debug.HasExited) {$debug.Kill()}
  }
}
