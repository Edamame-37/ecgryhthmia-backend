use rusqlite::Connection;

fn main() -> Result<(), rusqlite::Error> {
    println!("Membuka database terenkripsi...");
    let conn = Connection::open("database.db")?;
    
    // Setel kunci untuk mendekripsi database.db
    conn.execute_batch("PRAGMA key = 'ecgrhythmia-super-secret-key-2026';")?;

    println!("Mengekspor data ke format plaintext (database_decrypted.db)...");
    
    // Jika file sudah ada, hapus agar tidak bentrok
    let _ = std::fs::remove_file("database_decrypted.db");

    // Lakukan ekspor menggunakan fungsi bawaan sqlcipher
    conn.execute_batch(
        "ATTACH DATABASE 'database_decrypted.db' AS plaintext KEY '';
         SELECT sqlcipher_export('plaintext');
         DETACH DATABASE plaintext;"
    )?;

    println!("Selesai! File berhasil didekripsi menjadi 'database_decrypted.db'");
    Ok(())
}
