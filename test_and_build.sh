#!/bin/bash
# test_and_build.sh
# Script untuk menjalankan pengujian unit & integrasi, menganalisis hasilnya, lalu melakukan build produksi di Linux.

set -e # Keluar jika ada perintah yang gagal

# Kode warna ANSI untuk output terminal
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color (Reset)

echo -e "${CYAN}=============================================${NC}"
echo -e "${CYAN}1. Menjalankan Pengujian (Unit & Integrasi)...${NC}"
echo -e "${CYAN}=============================================${NC}"

# Jalankan cargo test dan simpan outputnya (termasuk stderr)
# Catatan: Di Linux, jika OpenSSL diinstal secara sistem, cargo akan melink secara otomatis.
# Jika Anda butuh custom OPENSSL_DIR di Linux, silakan export sebelum menjalankan skrip ini.
test_output=$(cargo test 2>&1) || true

# Cetak hasil test ke konsol secara transparan
echo "$test_output"

# Analisis hasil test
# Ekstrak pola "test result: ok. X passed; Y failed" dari output
passed_counts=$(echo "$test_output" | grep -E -o "test result: ok\. [0-9]+ passed" | grep -E -o "[0-9]+")
failed_counts=$(echo "$test_output" | grep -E -o "test result:.* [0-9]+ failed" | grep -E -o "[0-9]+")

total_passed=0
for val in $passed_counts; do
    total_passed=$((total_passed + val))
done

total_failed=0
for val in $failed_counts; do
    total_failed=$((total_failed + val))
done

# Tampilkan hasil
echo -e "\n${YELLOW}=============================================${NC}"
echo -e "${YELLOW}HASIL PENGUJIAN AKHIR:${NC}"
echo -e "- Total Test Berhasil (Passed): ${GREEN}${total_passed}${NC}"
if [ "$total_failed" -gt 0 ]; then
    echo -e "- Total Test Gagal (Failed): ${RED}${total_failed}${NC}"
else
    echo -e "- Total Test Gagal (Failed): ${total_failed}"
fi
echo -e "${YELLOW}=============================================${NC}"

# Cek jika ada test yang gagal
if [ "$total_failed" -gt 0 ]; then
    echo -e "${RED}[ERROR] Ada pengujian yang gagal! Proses build & deploy dibatalkan.${NC}"
    exit 1
fi

# Cek kegagalan kompilasi pengujian (jika passed counts 0 tapi ada output error)
if [ "$total_passed" -eq 0 ]; then
    if echo "$test_output" | grep -q "error:"; then
        echo -e "${RED}[ERROR] Kompilasi pengujian gagal! Batalkan build.${NC}"
        exit 1
    fi
fi

# Lanjutkan ke Build jika semua test berhasil
echo -e "\n${CYAN}=============================================${NC}"
echo -e "${CYAN}2. Memulai Proses Build & Kompilasi Rilis...${NC}"
echo -e "${CYAN}=============================================${NC}"

cargo build --release

echo -e "\n${GREEN}=============================================${NC}"
echo -e "${GREEN}PROSES BERHASIL!${NC}"
echo -e "- Semua pengujian ($total_passed test) lolos."
echo -e "- Biner produksi telah dikompilasi di: target/release/ecg-backend"
echo -e "${GREEN}=============================================${NC}"
