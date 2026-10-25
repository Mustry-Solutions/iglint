SELECT pump_id, name, status
FROM pump
WHERE area = :area
ORDER BY name