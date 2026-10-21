import json


def status(request, session):
	return {"json": {"ok": True, "project": system.util.getProjectName()}}


def ingest(request, session):
	body = json.loads(request["postData"] or "{}")
	return {"json": {"received": len(body)}}