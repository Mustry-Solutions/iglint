def onBarcodeDataReceived(session, data, context):
	session.custom.lastScan = data.text