import re

with open("src/api/routes.rs", "r", encoding="utf-8") as f:
    content = f.read()

# 535: started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path
content = re.sub(
    r'started_at:\s*row\.started_at,\s*ended_at:\s*row\.ended_at,\s*file_path:\s*row\.file_path',
    r'started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path',
    content
)

# 544-545: id: row.id, device_id: row.device_id, patient_id: Some(row.patient_id), patient_name: row.patient_name,
# started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path
content = re.sub(
    r'id:\s*row\.id,\s*device_id:\s*row\.device_id,\s*patient_id:\s*row\.patient_id,\s*patient_name:\s*row\.patient_name,',
    r'id: row.id, device_id: row.device_id, patient_id: Some(row.patient_id), patient_name: row.patient_name,',
    content
)

# 619: gender: patient_res.gender.unwrap_or_default(), primary_doctor_id: patient_res.primary_doctor_id,
content = re.sub(
    r'gender:\s*patient_res\.gender,\s*primary_doctor_id:\s*patient_res\.primary_doctor_id,',
    r'gender: patient_res.gender.unwrap_or_default(), primary_doctor_id: patient_res.primary_doctor_id,',
    content
)

# 632: email: res.email, role: res.role, profile_photo: res.profile_photo
content = re.sub(
    r'email:\s*res\.email\.unwrap_or_default\(\),\s*role:\s*res\.role\.unwrap_or_default\(\),',
    r'email: res.email, role: res.role,',
    content
)

# 637-638: .account_id
content = re.sub(
    r'\.account_id\.unwrap_or_default\(\);',
    r'.account_id;',
    content
)

with open("src/api/routes.rs", "w", encoding="utf-8") as f:
    f.write(content)
