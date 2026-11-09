# Compila la versión final del lector y la instala para el usuario actual (sin permisos
# de administrador), con acceso directo en el escritorio y en el menú Inicio.
# Se puede volver a ejecutar para actualizar la app tras cambiar el código.
#
# Uso: clic derecho > "Ejecutar con PowerShell", o desde una terminal:
#   powershell -ExecutionPolicy Bypass -File instalar.ps1

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "Compilando la versión final (la primera vez tarda unos minutos)..."
# cargo escribe su progreso por la salida de errores: con "Stop", PowerShell 5.1 lo
# tomaría por un fallo. Lo que cuenta es su código de salida.
$ErrorActionPreference = "Continue"
cargo build --release -p comic-app
$ErrorActionPreference = "Stop"
if ($LASTEXITCODE -ne 0) { throw "La compilación ha fallado" }

# Donde Windows instala las apps de un solo usuario.
$destino = Join-Path $env:LOCALAPPDATA "Programs\LectorComics"
$exe = Join-Path $destino "LectorComics.exe"
New-Item -ItemType Directory -Force $destino | Out-Null
try {
    Copy-Item "target\release\comic-app.exe" $exe -Force
} catch {
    throw "No se pudo copiar el programa. Si el lector está abierto, ciérralo y vuelve a intentarlo."
}

$shell = New-Object -ComObject WScript.Shell
$carpetas = @([Environment]::GetFolderPath("Desktop"), [Environment]::GetFolderPath("Programs"))
foreach ($carpeta in $carpetas) {
    $acceso = $shell.CreateShortcut((Join-Path $carpeta "Lector de cómics.lnk"))
    $acceso.TargetPath = $exe
    $acceso.WorkingDirectory = $destino
    $acceso.Description = "Lee y organiza tus cómics"
    $acceso.Save()
}

Write-Host "Instalado en $exe"
Write-Host "Accesos directos creados en el escritorio y en el menú Inicio."
