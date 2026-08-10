# test_and_build.ps1
# Script untuk menjalankan pengujian unit & integrasi, menganalisis hasilnya, lalu melakukan build produksi.

# Pastikan variabel OpenSSL diset untuk Windows agar tidak gagal melink ke database terenkripsi
$env:OPENSSL_DIR = "d:\Project\ecgrhythmia-backend\openssl-custom"
$env:OPENSSL_STATIC = "1"

Write-Host "=============================================" -ForegroundColor Cyan
Write-Host "1. Menjalankan Pengujian (Unit & Integrasi)..." -ForegroundColor Cyan
Write-Host "=============================================" -ForegroundColor Cyan

# Jalankan cargo test dan simpan outputnya
$testOutput = cargo test 2>&1

# Cetak output test ke konsol agar transparan
$testOutput | Out-String | Write-Host

# Analisis hasil test
# Cari pola "test result: ok. X passed; Y failed" di output
$passedMatches = $testOutput | Select-String -Pattern "test result: ok\. (\d+) passed"
$failedMatches = $testOutput | Select-String -Pattern "test result:.* (\d+) failed"

$totalPassed = 0
foreach ($match in $passedMatches) {
    if ($match.Matches.Groups[1].Value -match '\d+') {
        $totalPassed += [int]$match.Matches.Groups[1].Value
    }
}

$totalFailed = 0
foreach ($match in $failedMatches) {
    if ($match.Matches.Groups[1].Value -match '\d+') {
        $totalFailed += [int]$match.Matches.Groups[1].Value
    }
}

# Laporkan hasil pengujian sebelum build
Write-Host "`n=============================================" -ForegroundColor Yellow
Write-Host "HASIL PENGUJIAN AKHIR:" -ForegroundColor Yellow
Write-Host "- Total Test Berhasil (Passed): $totalPassed" -ForegroundColor Green
$failedColor = "Gray"
if ($totalFailed -gt 0) { $failedColor = "Red" }
Write-Host "- Total Test Gagal (Failed): $totalFailed" -ForegroundColor $failedColor
Write-Host "=============================================" -ForegroundColor Yellow

if ($totalFailed -gt 0) {
    Write-Host "[ERROR] Ada pengujian yang gagal! Proses build & deploy dibatalkan." -ForegroundColor Red
    exit 1
}

# Periksa jika kompilasi test itu sendiri gagal (jika passed matches 0 tapi output mengandung error)
if ($totalPassed -eq 0) {
    if ($testOutput -match "error:") {
        Write-Host "[ERROR] Kompilasi pengujian gagal! Batalkan build." -ForegroundColor Red
        exit 1
    }
}

# Lanjutkan ke Build jika semua test berhasil
Write-Host "`n=============================================" -ForegroundColor Cyan
Write-Host "2. Memulai Proses Build & Kompilasi Rilis..." -ForegroundColor Cyan
Write-Host "=============================================" -ForegroundColor Cyan

cargo build --release

if ($LASTEXITCODE -ne 0) {
    Write-Host "[ERROR] Proses build rilis gagal!" -ForegroundColor Red
    exit 1
}

Write-Host "`n=============================================" -ForegroundColor Green
Write-Host "PROSES BERHASIL!" -ForegroundColor Green
Write-Host "- Semua pengujian ($totalPassed test) lolos." -ForegroundColor Green
Write-Host "- Biner produksi telah dikompilasi di: target\release\ecg-backend.exe" -ForegroundColor Green
Write-Host "=============================================" -ForegroundColor Green
