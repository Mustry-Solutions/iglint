def doPost(request, session):
	return api.handlers.ingest(request, session)