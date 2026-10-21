POPUP_ID = "motorDetail"


def openMotor(tagPath):
	system.perspective.openPopup(POPUP_ID, "Popups/MotorDetail", params={"tagPath": tagPath}, modal=True)


def goHome():
	system.perspective.navigate(page="/")