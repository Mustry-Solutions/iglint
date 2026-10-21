def handleMessage(session, payload):
	session.custom.selectedLine = payload["line"]