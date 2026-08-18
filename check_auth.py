import psycopg2
DATABASE_URL = "postgresql://postgres.xzjxkplsgzcvdcjdhpcp:bapakkauperangbarengidf@aws-0-ap-northeast-1.pooler.supabase.com:5432/postgres?sslmode=require"
conn = psycopg2.connect(DATABASE_URL)
cur = conn.cursor()
cur.execute("SELECT count(*) FROM auth.users")
print("Auth users count:", cur.fetchone()[0])
