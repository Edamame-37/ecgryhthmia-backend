import os
import json
import sqlite3
import uuid
import datetime
import glob
import numpy as np
from pathlib import Path

# Config
SESSION_DIR = r"c:\ecgrhythmia\ecgrhythmia-backend\manual_device_record\poli_11Agustus\session_11082026_001958"
RECORDS_LOCAL_DIR = r"c:\ecgrhythmia\ecgrhythmia-backend\records\records_local"
DB_PATH = r"c:\ecgrhythmia\ecgrhythmia-backend\legacy_sqlite_db\databae-local\database_decrypted.db"
DEVICE_ID = "dev_1786049246081"
PATIENT_ID = "pat000000000002"

def generate_session_id():
    return f"ses{uuid.uuid4().hex[:12]}"

def generate_frame_id():
    return f"frm{uuid.uuid4().hex[:12]}"

def main():
    # 1. Ensure output dir exists
    os.makedirs(RECORDS_LOCAL_DIR, exist_ok=True)
    
    # 2. Read session metadata
    session_json_path = os.path.join(SESSION_DIR, "session.json")
    with open(session_json_path, 'r') as f:
        session_meta = json.load(f)
    
    started_at = session_meta.get("started_at", datetime.datetime.now().isoformat())
    ended_at = session_meta.get("ended_at")
    
    session_id = generate_session_id()
    jsonl_filename = f"{session_id}.jsonl"
    jsonl_path = os.path.join(RECORDS_LOCAL_DIR, jsonl_filename)
    
    # 3. Find all frames
    calibrated_dir = os.path.join(SESSION_DIR, "calibrated")
    predictions_dir = os.path.join(SESSION_DIR, "predictions")
    
    npy_files = sorted(glob.glob(os.path.join(calibrated_dir, "frame_*_mv.npy")))
    
    jsonl_lines = []
    frame_db_records = []
    
    # Base timestamp
    try:
        base_time = datetime.datetime.fromisoformat(started_at.replace("Z", "+00:00"))
    except ValueError:
        base_time = datetime.datetime.now()
        
    duration_s = session_meta.get("duration_per_frame_s", 10.0)
    
    for i, npy_file in enumerate(npy_files):
        filename = os.path.basename(npy_file)
        frame_num_str = filename.replace("frame_", "").replace("_mv.npy", "")
        frame_id_str = frame_num_str
        
        # Load NPY
        samples = np.load(npy_file, allow_pickle=False)
        
        # Load Prediction
        pred_file = os.path.join(predictions_dir, f"frame_{frame_num_str}_prediction.json")
        prediction_data = {}
        if os.path.exists(pred_file):
            with open(pred_file, 'r') as f:
                prediction_data = json.load(f)
                
        # Build Payload
        message_id = f"{DEVICE_ID}-{session_id}-frame_{frame_id_str}"
        created_at = (base_time + datetime.timedelta(seconds=i * duration_s)).isoformat()
        
        pred_label = prediction_data.get("prediction", prediction_data.get("label", "Unknown"))
        
        payload = {
            "schema_version": 1,
            "message_id": message_id,
            "device_id": DEVICE_ID,
            "session_id": session_id,
            "frame_id": frame_id_str,
            "created_at": created_at,
            "sampling_rate_hz": session_meta.get("sampling_rate_hz", 250.0),
            "duration_s": duration_s,
            "unit": session_meta.get("unit", "mV"),
            "shape": list(samples.shape),
            "channel_order": session_meta.get("channel_order", []),
            "validation": {
                "status": "PASS",
                "warnings": []
            },
            "ecg": {
                "samples": samples.tolist()
            },
            "prediction": {
                "status": prediction_data.get("status", "PASS"),
                "label": pred_label,
                "confidence_percent": prediction_data.get("confidence_percent", 0),
                "probabilities": prediction_data.get("probabilities", {}),
                "threshold": prediction_data.get("threshold", 0.5),
                "latency_ms": prediction_data.get("latency_ms", 0),
                "runtime": prediction_data.get("runtime", "")
            },
            "system": {
                "cpu_usage_percent": 50.0,
                "memory_usage_percent": 50.0,
                "memory_usage_mb": 1000,
                "cpu_temperature_c": 50.0,
                "uptime_s": 1000
            },
            "stress_test": {"enabled": False, "frame_counter": i},
            "network": {"mqtt_connected": True}
        }
        
        jsonl_lines.append(json.dumps(payload))
        
        # Prepare DB record
        f_id = generate_frame_id()
        time_interval = f"frame_{frame_id_str}"
        doc_class = pred_label
        
        frame_db_records.append((f_id, session_id, time_interval, None, doc_class))

    # 4. Write JSONL
    with open(jsonl_path, 'w') as f:
        f.write('\n'.join(jsonl_lines))
        
    print(f"Written {len(jsonl_lines)} frames to {jsonl_path}")
    
    # 5. Insert into SQLite
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    
    # Insert session
    relative_file_path = f"records_local/{jsonl_filename}"
    cursor.execute("""
        INSERT INTO sessions (id, device_id, patient_id, started_at, ended_at, file_path)
        VALUES (?, ?, ?, ?, ?, ?)
    """, (session_id, DEVICE_ID, PATIENT_ID, started_at, ended_at, relative_file_path))
    
    # Insert frames
    cursor.executemany("""
        INSERT INTO frame_records (id, session_id, time_interval, confirmation, doc_classification)
        VALUES (?, ?, ?, ?, ?)
    """, frame_db_records)
    
    conn.commit()
    conn.close()
    
    print("Database insert complete.")

if __name__ == "__main__":
    main()
