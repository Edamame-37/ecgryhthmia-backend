import re

with open("src/api/routes.rs", "r", encoding="utf-8") as f:
    content = f.read()

# 572: &path.file_path -> path.file_path.as_deref().unwrap_or_default()
content = content.replace("fs::read_to_string(&path.file_path)", "fs::read_to_string(path.file_path.as_deref().unwrap_or_default())")

with open("src/api/routes.rs", "w", encoding="utf-8") as f:
    f.write(content)
