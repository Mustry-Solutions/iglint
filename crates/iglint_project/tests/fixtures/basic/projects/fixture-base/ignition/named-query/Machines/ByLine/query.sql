SELECT machine_id, name, state
FROM machine
WHERE line = :line
ORDER BY name