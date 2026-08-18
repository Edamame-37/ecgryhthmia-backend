import psycopg2

DATABASE_URL = "postgresql://postgres.xzjxkplsgzcvdcjdhpcp:bapakkauperangbarengidf@aws-0-ap-northeast-1.pooler.supabase.com:5432/postgres?sslmode=require"
conn = psycopg2.connect(DATABASE_URL)
cursor = conn.cursor()

cursor.execute("SELECT id, name FROM devices")
print("Devices:")
for row in cursor.fetchall():
    print(row)

cursor.execute("SELECT device_id, count(*) FROM sessions GROUP BY device_id")
print("\nSession Device Count:")
for row in cursor.fetchall():
    print(row)
