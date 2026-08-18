import os
import psycopg2

db_url = "postgresql://postgres.xzjxkplsgzcvdcjdhpcp:bapakkauperangbarengidf@aws-0-ap-northeast-1.pooler.supabase.com:5432/postgres?sslmode=require"
try:
    conn = psycopg2.connect(db_url)
    cur = conn.cursor()
    cur.execute("SELECT id, role FROM accounts LIMIT 5;")
    for row in cur.fetchall():
        print(row)
    
    cur.execute("SELECT id, account_id FROM patients LIMIT 5;")
    for row in cur.fetchall():
        print(f"Patient: {row}")
except Exception as e:
    print(e)
