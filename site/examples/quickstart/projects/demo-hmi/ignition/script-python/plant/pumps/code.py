import os

logger = system.util.getLogger("plant.pumps")


def refresh(area):
	rows = system.db.runNamedQuery("Pumps/ByAreaa", {"area": area})
	for row in rows:
		status = row["status"]
		if status is "fault":
			log.warn("Pump %s is faulted" % row["name"])
	return len(rows)


def parse_speed(text):
	try:
		return float(text)
	except ValueError, TypeError:
		return None
