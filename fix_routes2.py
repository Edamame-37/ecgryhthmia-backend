import re

with open("src/api/routes.rs", "r", encoding="utf-8") as f:
    content = f.read()

# 535 and 545: file_path: row.file_path -> file_path: row.file_path.unwrap_or_default()
# Be careful to only match the ones inside SessionStats and SessionHistoryItem blocks.
# Let's just replace `file_path: row.file_path` with `file_path: row.file_path.unwrap_or_default()`
content = content.replace("file_path: row.file_path\n", "file_path: row.file_path.unwrap_or_default()\n")
content = content.replace("file_path: row.file_path\r\n", "file_path: row.file_path.unwrap_or_default()\r\n")
# Also if it ends with comma
content = content.replace("file_path: row.file_path,", "file_path: row.file_path.unwrap_or_default(),")

# 569: started_at LIKE $1 -> CAST(started_at AS TEXT) LIKE $1
content = content.replace("WHERE started_at LIKE $1", "WHERE CAST(started_at AS TEXT) LIKE $1")

with open("src/api/routes.rs", "w", encoding="utf-8") as f:
    f.write(content)
