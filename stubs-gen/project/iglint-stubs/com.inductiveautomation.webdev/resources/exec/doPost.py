def doPost(request, session):
	# Executes the posted Jython source and returns its `result` variable as
	# JSON, or the traceback. Only reachable inside the throwaway stubs-gen
	# container.
	import system
	import traceback
	ns = {"system": system, "request": request}
	try:
		exec request["postData"] in ns
	except:
		return {"json": {"error": traceback.format_exc()}}
	return {"json": ns.get("result")}
