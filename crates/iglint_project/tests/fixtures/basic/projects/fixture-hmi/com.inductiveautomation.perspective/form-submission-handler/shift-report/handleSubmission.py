def handleSubmission(session, name, data, formContext, sessionContext, retry):
	system.db.runNamedQuery("Machines/SetState", {"table": "machine", "id": data["id"], "state": data["state"]})