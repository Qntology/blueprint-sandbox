"""다국어 N:N 검색 평가 질의 (12개 언어 질의 × 데이터 언어 ko · en · ja · de).

정답 id(KKS · 태그 · 문서 번호)는 데이터 언어와 상관없이 같습니다. 한국어 · 영어 질의는 엔진의 기존 평가 묶음
(core/src/search/mod.rs 의 EVAL_*)과 같은 글이고, 나머지 10개 언어는 같은 뜻을 그 언어 화자가 검색창에 칠 꼴로 옮긴 것입니다.

묶음 (split):
  dev    발전소 원장 질의 · 지금 계측(P2) 질의 · 따옴표 이름 질의 — 엔진 규칙을 고칠 때 보는 묶음
  held   데이터센터 원장 질의 · 지금 계측(D2) 질의, 발전소 · 데이터센터 뜻풀이 — 규칙을 고칠 때 보지 않고 마지막에 한 번 잽니다
식별자만 있는 질의(언어 없음)는 lang "id" 로 데이터 언어마다 한 번씩 둡니다.

따옴표 이름 질의(quoted)는 데이터 언어의 원장 이름을 질의 언어 문장 안에 따옴표로 넣습니다
(logis-center c0e801c1: 데이터 값의 문자 체계가 질의 언어 판정을 덮지 않아야 함).

    python scripts/search_ml/queries.py           # core/fixtures/search_ml/queries.json
    python scripts/search_ml/queries.py --check
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "core" / "fixtures" / "search_ml" / "queries.json"
LANGS = ["ko", "en", "de", "fr", "es", "it", "pt", "nl", "cs", "ar", "ja", "zh-hans"]
DATA_LANGS = ["ko", "en", "ja", "de"]

# 같은 뜻의 질의 (12개 언어). 키: 묶음 이름.
T = {
    "valves_b103_f2": {
        "ko": "103동 2층 밸브", "en": "valves on floor 2 of building 103",
        "de": "Ventile in Gebäude 103 im 2. Stock", "fr": "vannes du bâtiment 103 au 2e étage",
        "es": "válvulas del edificio 103 en la planta 2", "it": "valvole dell'edificio 103 al secondo piano",
        "pt": "válvulas do edifício 103 no 2º andar", "nl": "kleppen in gebouw 103 op de 2e verdieping",
        "cs": "ventily v budově 103 ve 2. patře", "ar": "الصمامات في المبنى 103 الطابق الثاني",
        "ja": "103号棟2階のバルブ", "zh-hans": "103号楼2层的阀门",
    },
    "fw_bypass": {
        "ko": "급수 바이패스", "en": "feedwater bypass",
        "de": "Speisewasser-Bypass", "fr": "bypass eau alimentaire", "es": "bypass de agua de alimentación",
        "it": "bypass acqua di alimento", "pt": "bypass de água de alimentação", "nl": "voedingswater bypass",
        "cs": "obtok napájecí vody", "ar": "الممر الجانبي لمياه التغذية", "ja": "給水バイパス", "zh-hans": "给水旁路",
    },
    "sh_temp_sensor": {
        "ko": "과열기 온도 센서", "en": "superheater temperature sensor",
        "de": "Temperatursensor Überhitzer", "fr": "capteur de température du surchauffeur",
        "es": "sensor de temperatura del sobrecalentador", "it": "sensore di temperatura del surriscaldatore",
        "pt": "sensor de temperatura do superaquecedor", "nl": "temperatuursensor oververhitter",
        "cs": "teplotní čidlo přehříváku", "ar": "حساس حرارة المحمص", "ja": "過熱器の温度センサー", "zh-hans": "过热器温度传感器",
    },
    "fcv_upstream": {
        "ko": "주급수 유량조절밸브 상류 1홉", "en": "main feedwater flow control valve 1 hop upstream",
        "de": "1 Hop stromaufwärts vom Hauptspeisewasser-Durchflussregelventil",
        "fr": "1 saut en amont de la vanne de régulation de débit d'eau alimentaire principale",
        "es": "1 salto aguas arriba de la válvula de control de caudal de agua de alimentación principal",
        "it": "1 salto a monte della valvola di regolazione portata acqua alimento principale",
        "pt": "1 salto a montante da válvula de controle de vazão de água de alimentação principal",
        "nl": "1 hop stroomopwaarts van de hoofdvoedingswater debietregelklep",
        "cs": "1 skok proti proudu od hlavního regulačního ventilu průtoku napájecí vody",
        "ar": "أعلى التيار 1 قفزة من صمام التحكم في تدفق مياه التغذية الرئيسي",
        "ja": "主給水流量調節弁の上流1ホップ", "zh-hans": "主给水流量调节阀上游1跳",
    },
    "basement_pumps": {
        "ko": "지하 펌프", "en": "basement pumps",
        "de": "Pumpen im Keller", "fr": "pompes au sous-sol", "es": "bombas del sótano", "it": "pompe nel seminterrato",
        "pt": "bombas no subsolo", "nl": "pompen in de kelder", "cs": "čerpadla v suterénu", "ar": "المضخات في القبو",
        "ja": "地下のポンプ", "zh-hans": "地下的泵",
    },
    "pending_moc": {
        "ko": "승인 대기 MOC", "en": "pending MOC",
        "de": "MOC mit ausstehender Genehmigung", "fr": "MOC en attente d'approbation", "es": "MOC pendientes de aprobación",
        "it": "MOC in attesa di approvazione", "pt": "MOC pendentes de aprovação", "nl": "MOC in afwachting van goedkeuring",
        "cs": "MOC čekající na schválení", "ar": "طلبات MOC بانتظار الموافقة", "ja": "承認待ちのMOC", "zh-hans": "待审批的MOC",
    },
    "bearing_vib_proc": {
        "ko": "베어링 진동 절차서", "en": "bearing vibration procedure",
        "de": "Verfahren bei Lagerschwingung", "fr": "procédure vibration des paliers", "es": "procedimiento de vibración de cojinetes",
        "it": "procedura vibrazione cuscinetti", "pt": "procedimento de vibração do mancal", "nl": "procedure lagertrilling",
        "cs": "postup při vibracích ložiska", "ar": "إجراء اهتزاز المحامل", "ja": "軸受振動の手順書", "zh-hans": "轴承振动处理规程",
    },
    "alarm_600": {
        "ko": "경보 한계 600도 이상 센서", "en": "sensors with alarm limit above 600 degrees",
        "de": "Sensoren mit Alarmgrenze über 600 Grad", "fr": "capteurs avec seuil d'alarme supérieur à 600 degrés",
        "es": "sensores con límite de alarma superior a 600 grados", "it": "sensori con soglia di allarme sopra 600 gradi",
        "pt": "sensores com limite de alarme acima de 600 graus", "nl": "sensoren met alarmgrens boven 600 graden",
        "cs": "čidla s mezí alarmu nad 600 stupňů", "ar": "الحساسات ذات حد إنذار أعلى من 600 درجة",
        "ja": "警報限界600度以上のセンサー", "zh-hans": "报警限值600度以上的传感器",
    },
    "b105_drawings": {
        "ko": "105동 도면", "en": "building 105 drawings",
        "de": "Zeichnungen Gebäude 105", "fr": "plans du bâtiment 105", "es": "planos del edificio 105",
        "it": "disegni dell'edificio 105", "pt": "desenhos do edifício 105", "nl": "tekeningen van gebouw 105",
        "cs": "výkresy budovy 105", "ar": "مخططات المبنى 105", "ja": "105号棟の図面", "zh-hans": "105号楼图纸",
    },
    "id_fan": {
        "ko": "유인통풍기", "en": "ID fan",
        "de": "Saugzuggebläse", "fr": "ventilateur de tirage induit", "es": "ventilador de tiro inducido",
        "it": "ventilatore a tiraggio indotto", "pt": "ventilador de tiragem induzida", "nl": "trekventilator",
        "cs": "kouřový ventilátor", "ar": "مروحة السحب المستحث", "ja": "誘引通風機", "zh-hans": "引风机",
    },
    "main_steam_pressure": {
        "ko": "주증기 압력", "en": "main steam pressure",
        "de": "Frischdampfdruck", "fr": "pression de vapeur principale", "es": "presión de vapor principal",
        "it": "pressione vapore principale", "pt": "pressão de vapor principal", "nl": "hoofdstoomdruk",
        "cs": "tlak ostré páry", "ar": "ضغط البخار الرئيسي", "ja": "主蒸気圧力", "zh-hans": "主蒸汽压力",
    },
    "fwp_high_pressure": {
        "ko": "103동 압력 높은 급수펌프", "en": "feedwater pump with high pressure in building 103",
        "de": "Speisewasserpumpe mit hohem Druck in Gebäude 103",
        "fr": "pompe alimentaire avec pression élevée dans le bâtiment 103",
        "es": "bomba de alimentación con presión alta en el edificio 103",
        "it": "pompa di alimento con pressione alta nell'edificio 103",
        "pt": "bomba de alimentação com pressão alta no edifício 103", "nl": "voedingspomp met hoge druk in gebouw 103",
        "cs": "napájecí čerpadlo s vysokým tlakem v budově 103", "ar": "مضخة التغذية ذات الضغط المرتفع في المبنى 103",
        "ja": "103号棟の圧力が高い給水ポンプ", "zh-hans": "103号楼压力高的给水泵",
    },
    "sh_abnormal": {
        "ko": "보일러동 지하 출구온도 비정상 과열기",
        "en": "abnormal outlet temperature superheater in the boiler building basement",
        "de": "Überhitzer mit abnormaler Austrittstemperatur im Keller des Kesselhauses",
        "fr": "surchauffeur avec température de sortie anormale au sous-sol du bâtiment chaudière",
        "es": "sobrecalentador con temperatura de salida anormal en el sótano del edificio de calderas",
        "it": "surriscaldatore con temperatura di uscita anomala nel seminterrato dell'edificio caldaia",
        "pt": "superaquecedor com temperatura de saída anormal no subsolo do edifício da caldeira",
        "nl": "oververhitter met abnormale uitlaattemperatuur in de kelder van het ketelhuis",
        "cs": "přehřívák s abnormální výstupní teplotou v suterénu kotelny",
        "ar": "محمص بدرجة حرارة مخرج غير طبيعية في قبو مبنى الغلاية",
        "ja": "ボイラー棟地下の出口温度異常の過熱器", "zh-hans": "锅炉房地下出口温度异常的过热器",
    },
    "stuck_id": {
        "ko": "10LABI0AA102 고착", "en": "10LABI0AA102 stuck", "de": "10LABI0AA102 festsitzend", "fr": "10LABI0AA102 bloquée",
        "es": "10LABI0AA102 atascada", "it": "10LABI0AA102 bloccata", "pt": "10LABI0AA102 travada", "nl": "10LABI0AA102 vastgelopen",
        "cs": "10LABI0AA102 zaseknutý", "ar": "10LABI0AA102 عالق", "ja": "10LABI0AA102 固着", "zh-hans": "10LABI0AA102 卡涩",
    },
    "stuck_valves": {
        "ko": "고착된 밸브", "en": "stuck valves", "de": "festsitzende Ventile", "fr": "vannes bloquées", "es": "válvulas atascadas",
        "it": "valvole bloccate", "pt": "válvulas travadas", "nl": "vastgelopen kleppen", "cs": "zaseknuté ventily",
        "ar": "الصمامات العالقة", "ja": "固着したバルブ", "zh-hans": "卡涩的阀门",
    },
    "low_fw_flow": {
        "ko": "유량 낮은 급수", "en": "low feedwater flow", "de": "niedriger Speisewasserdurchfluss",
        "fr": "débit d'eau alimentaire faible", "es": "caudal de agua de alimentación bajo", "it": "portata acqua di alimento bassa",
        "pt": "vazão de água de alimentação baixa", "nl": "laag voedingswaterdebiet", "cs": "nízký průtok napájecí vody",
        "ar": "تدفق مياه التغذية منخفض", "ja": "給水流量が低い", "zh-hans": "给水流量低",
    },
    "hot_sensors": {
        "ko": "온도 높은 센서", "en": "sensors with high temperature", "de": "Sensoren mit hoher Temperatur",
        "fr": "capteurs avec température élevée", "es": "sensores con temperatura alta", "it": "sensori con temperatura alta",
        "pt": "sensores com temperatura alta", "nl": "sensoren met hoge temperatuur", "cs": "čidla s vysokou teplotou",
        "ar": "الحساسات ذات درجة الحرارة المرتفعة", "ja": "温度が高いセンサー", "zh-hans": "温度高的传感器",
    },
    "abnormal_b103": {
        "ko": "103동 비정상 설비", "en": "abnormal equipment in building 103", "de": "abnormale Geräte in Gebäude 103",
        "fr": "équipements anormaux dans le bâtiment 103", "es": "equipos anormales en el edificio 103",
        "it": "apparecchiature anomale nell'edificio 103", "pt": "equipamentos anormais no edifício 103",
        "nl": "afwijkende apparatuur in gebouw 103", "cs": "abnormální zařízení v budově 103",
        "ar": "المعدات غير الطبيعية في المبنى 103", "ja": "103号棟の異常な設備", "zh-hans": "103号楼异常设备",
    },
    # 데이터센터 (held)
    "cdu_bypass": {
        "ko": "CDU 우회밸브", "en": "CDU bypass valve", "de": "CDU-Bypassventil", "fr": "vanne de bypass CDU",
        "es": "válvula de bypass del CDU", "it": "valvola di bypass CDU", "pt": "válvula de bypass da CDU", "nl": "CDU-bypassklep",
        "cs": "obtokový ventil CDU", "ar": "صمام تجاوز CDU", "ja": "CDUバイパス弁", "zh-hans": "CDU旁通阀",
    },
    "gpu_junction": {
        "ko": "GPU 정션 온도 센서", "en": "GPU junction temperature sensor", "de": "GPU-Sperrschichttemperatursensor",
        "fr": "capteur de température de jonction GPU", "es": "sensor de temperatura de unión de la GPU",
        "it": "sensore temperatura di giunzione GPU", "pt": "sensor de temperatura de junção da GPU",
        "nl": "GPU-junctietemperatuursensor", "cs": "čidlo teploty přechodu GPU", "ar": "حساس درجة حرارة وصلة GPU",
        "ja": "GPUジャンクション温度センサー", "zh-hans": "GPU结温传感器",
    },
    "pod_a_manifold": {
        "ko": "Pod A 매니폴드", "en": "Pod A manifold", "de": "Verteiler Pod A", "fr": "collecteur du Pod A", "es": "colector del Pod A",
        "it": "collettore Pod A", "pt": "coletor do Pod A", "nl": "verdeler Pod A", "cs": "rozdělovač Pod A", "ar": "مشعب Pod A",
        "ja": "Pod Aのマニホールド", "zh-hans": "Pod A歧管",
    },
    "battery_cell_temp": {
        "ko": "배터리 셀 온도", "en": "battery cell temperature", "de": "Batteriezellentemperatur",
        "fr": "température des cellules de batterie", "es": "temperatura de celda de batería", "it": "temperatura cella batteria",
        "pt": "temperatura da célula da bateria", "nl": "batterijceltemperatuur", "cs": "teplota článků baterie",
        "ar": "درجة حرارة خلايا البطارية", "ja": "バッテリーセル温度", "zh-hans": "电池单体温度",
    },
    "cooling_tower_fan": {
        "ko": "냉각탑 팬", "en": "cooling tower fan", "de": "Kühlturmventilator", "fr": "ventilateur de tour de refroidissement",
        "es": "ventilador de la torre de refrigeración", "it": "ventilatore della torre di raffreddamento",
        "pt": "ventilador da torre de resfriamento", "nl": "koeltorenventilator", "cs": "ventilátor chladicí věže",
        "ar": "مروحة برج التبريد", "ja": "冷却塔ファン", "zh-hans": "冷却塔风机",
    },
    "thermal_runaway_proc": {
        "ko": "열폭주 절차서", "en": "thermal runaway procedure", "de": "Verfahren bei thermischem Durchgehen",
        "fr": "procédure d'emballement thermique", "es": "procedimiento de fuga térmica", "it": "procedura di fuga termica",
        "pt": "procedimento de fuga térmica", "nl": "procedure thermische runaway", "cs": "postup při tepelném úniku",
        "ar": "إجراء الانفلات الحراري", "ja": "熱暴走の手順書", "zh-hans": "热失控处置规程",
    },
    "pending_mop": {
        "ko": "승인 대기 MOP", "en": "pending MOP", "de": "MOP mit ausstehender Genehmigung", "fr": "MOP en attente d'approbation",
        "es": "MOP pendientes de aprobación", "it": "MOP in attesa di approvazione", "pt": "MOP pendentes de aprovação",
        "nl": "MOP in afwachting van goedkeuring", "cs": "MOP čekající na schválení", "ar": "طلبات MOP بانتظار الموافقة",
        "ja": "承認待ちのMOP", "zh-hans": "待审批的MOP",
    },
    "hot_gpu_racks": {
        "ko": "온도 높은 GPU 랙", "en": "GPU racks with high temperature", "de": "GPU-Racks mit hoher Temperatur",
        "fr": "baies GPU avec température élevée", "es": "racks de GPU con temperatura alta", "it": "rack GPU con temperatura alta",
        "pt": "racks de GPU com temperatura alta", "nl": "GPU-racks met hoge temperatuur", "cs": "GPU racky s vysokou teplotou",
        "ar": "رفوف GPU ذات درجة الحرارة المرتفعة", "ja": "温度が高いGPUラック", "zh-hans": "温度高的GPU机柜",
    },
    "pump_high_pressure_b103": {
        "ko": "103동 압력 높은 펌프", "en": "pump with high pressure in building 103", "de": "Pumpe mit hohem Druck in Gebäude 103",
        "fr": "pompe avec pression élevée dans le bâtiment 103", "es": "bomba con presión alta en el edificio 103",
        "it": "pompa con pressione alta nell'edificio 103", "pt": "bomba com pressão alta no edifício 103",
        "nl": "pomp met hoge druk in gebouw 103", "cs": "čerpadlo s vysokým tlakem v budově 103",
        "ar": "مضخة ذات ضغط مرتفع في المبنى 103", "ja": "103号棟の圧力が高いポンプ", "zh-hans": "103号楼压力高的泵",
    },
    "low_coolant_flow": {
        "ko": "유량 낮은 냉각수", "en": "low coolant flow", "de": "niedriger Kühlmitteldurchfluss",
        "fr": "débit de liquide de refroidissement faible", "es": "caudal de refrigerante bajo", "it": "portata del refrigerante bassa",
        "pt": "vazão de refrigerante baixa", "nl": "laag koelmiddeldebiet", "cs": "nízký průtok chladiva",
        "ar": "تدفق سائل التبريد منخفض", "ja": "冷却水流量が低い", "zh-hans": "冷却水流量低",
    },
    # 뜻풀이 (held)
    "pp_feed_pump": {
        "ko": "보일러에 물을 보내는 펌프", "en": "boiler feed pump", "de": "Pumpe, die Wasser zum Kessel fördert",
        "fr": "pompe qui envoie l'eau à la chaudière", "es": "bomba que envía agua a la caldera", "it": "pompa che manda acqua alla caldaia",
        "pt": "bomba que envia água para a caldeira", "nl": "pomp die water naar de ketel stuurt",
        "cs": "čerpadlo, které dodává vodu do kotle", "ar": "المضخة التي ترسل الماء إلى الغلاية",
        "ja": "ボイラーに水を送るポンプ", "zh-hans": "向锅炉送水的泵",
    },
    "pp_smoke_fan": {
        "ko": "연기를 빨아내는 팬", "en": "induced draft fan", "de": "Ventilator, der den Rauch absaugt",
        "fr": "ventilateur qui aspire la fumée", "es": "ventilador que aspira el humo", "it": "ventilatore che aspira i fumi",
        "pt": "ventilador que aspira a fumaça", "nl": "ventilator die de rook afzuigt", "cs": "ventilátor, který odsává kouř",
        "ar": "المروحة التي تسحب الدخان", "ja": "煙を吸い出すファン", "zh-hans": "抽出烟气的风机",
    },
    "pp_coal_powder": {
        "ko": "석탄을 가루로 만드는 설비", "en": "coal pulverizer", "de": "Anlage, die Kohle zu Pulver mahlt",
        "fr": "équipement qui réduit le charbon en poudre", "es": "equipo que convierte el carbón en polvo",
        "it": "impianto che riduce il carbone in polvere", "pt": "equipamento que transforma o carvão em pó",
        "nl": "installatie die kolen tot poeder maalt", "cs": "zařízení, které mele uhlí na prášek",
        "ar": "المعدة التي تطحن الفحم إلى مسحوق", "ja": "石炭を粉にする設備", "zh-hans": "把煤磨成粉的设备",
    },
    "pp_chimney": {
        "ko": "굴뚝", "en": "chimney", "de": "Kamin", "fr": "cheminée", "es": "chimenea", "it": "ciminiera", "pt": "chaminé",
        "nl": "schoorsteen", "cs": "komín", "ar": "المدخنة", "ja": "煙突", "zh-hans": "烟囱",
    },
    "pp_reheat": {
        "ko": "증기를 다시 데우는 설비", "en": "steam reheating", "de": "Anlage, die den Dampf wieder erhitzt",
        "fr": "équipement qui réchauffe à nouveau la vapeur", "es": "equipo que vuelve a calentar el vapor",
        "it": "impianto che riscalda di nuovo il vapore", "pt": "equipamento que reaquece o vapor",
        "nl": "installatie die de stoom opnieuw verhit", "cs": "zařízení, které znovu ohřívá páru",
        "ar": "المعدة التي تعيد تسخين البخار", "ja": "蒸気を再び温める設備", "zh-hans": "再次加热蒸汽的设备",
    },
    "pp_shaft_shake": {
        "ko": "터빈 축 떨림", "en": "turbine shaft vibration", "de": "Schwingen der Turbinenwelle",
        "fr": "tremblement de l'arbre de turbine", "es": "temblor del eje de la turbina", "it": "tremolio dell'albero della turbina",
        "pt": "tremor do eixo da turbina", "nl": "trillen van de turbineas", "cs": "chvění hřídele turbíny",
        "ar": "ارتجاف عمود التوربين", "ja": "タービン軸の揺れ", "zh-hans": "汽轮机轴抖动",
    },
    "pp_generated": {
        "ko": "발전량", "en": "power generated", "de": "erzeugte Strommenge", "fr": "quantité d'électricité produite",
        "es": "cantidad de energía generada", "it": "energia prodotta", "pt": "quantidade de energia gerada",
        "nl": "opgewekte hoeveelheid stroom", "cs": "množství vyrobené elektřiny", "ar": "كمية الكهرباء المولدة",
        "ja": "発電量", "zh-hans": "发电量",
    },
    "pp_sox": {
        "ko": "황산화물 배출", "en": "sulfur dioxide emissions", "de": "Schwefeloxid-Emissionen",
        "fr": "émissions d'oxydes de soufre", "es": "emisiones de óxidos de azufre", "it": "emissioni di ossidi di zolfo",
        "pt": "emissões de óxidos de enxofre", "nl": "uitstoot van zwaveloxiden", "cs": "emise oxidů síry",
        "ar": "انبعاثات أكاسيد الكبريت", "ja": "硫黄酸化物の排出", "zh-hans": "硫氧化物排放",
    },
    "pp_battery_risk": {
        "ko": "배터리 폭발 위험 절차", "en": "battery fire risk procedure", "de": "Verfahren bei Explosionsgefahr der Batterie",
        "fr": "procédure risque d'explosion de batterie", "es": "procedimiento de riesgo de explosión de batería",
        "it": "procedura rischio esplosione batteria", "pt": "procedimento de risco de explosão da bateria",
        "nl": "procedure explosiegevaar batterij", "cs": "postup při nebezpečí výbuchu baterie",
        "ar": "إجراء خطر انفجار البطارية", "ja": "バッテリー爆発危険の手順", "zh-hans": "电池爆炸风险处置程序",
    },
    "pp_fire_gas": {
        "ko": "불 끄는 가스 용기", "en": "fire suppression gas tank", "de": "Gasbehälter zum Feuerlöschen",
        "fr": "bouteille de gaz pour éteindre le feu", "es": "cilindro de gas para apagar incendios",
        "it": "bombola di gas per spegnere il fuoco", "pt": "cilindro de gás para apagar incêndios", "nl": "gasfles om brand te blussen",
        "cs": "plynová lahev k hašení požáru", "ar": "أسطوانة غاز لإطفاء الحريق", "ja": "火を消すガスの容器", "zh-hans": "灭火用的气体钢瓶",
    },
    "pp_optics_temp": {
        "ko": "광모듈 온도", "en": "network optics temperature", "de": "Temperatur des optischen Moduls",
        "fr": "température du module optique", "es": "temperatura del módulo óptico", "it": "temperatura del modulo ottico",
        "pt": "temperatura do módulo óptico", "nl": "temperatuur van de optische module", "cs": "teplota optického modulu",
        "ar": "درجة حرارة الوحدة الضوئية", "ja": "光モジュールの温度", "zh-hans": "光模块温度",
    },
}

# 따옴표 이름 틀: {x} 에 데이터 언어의 원장 이름
QUOTED = {
    "q_downstream": {
        "x": {"ko": "주급수펌프 A", "en": "Main Feedwater Pump A", "ja": "主給水ポンプA", "de": "Hauptspeisewasserpumpe A"},
        "expect": [("10LAB20AA102", 2)],
        "frame": {
            "ko": "'{x}' 하류 1홉", "en": "1 hop downstream of '{x}'", "de": "1 Hop stromabwärts von „{x}“",
            "fr": "1 saut en aval de « {x} »", "es": "1 salto aguas abajo de «{x}»", "it": "1 salto a valle di «{x}»",
            "pt": "1 salto a jusante de \"{x}\"", "nl": "1 hop stroomafwaarts van '{x}'", "cs": "1 skok po proudu od „{x}“",
            "ar": "قفزة 1 أسفل التيار من \"{x}\"", "ja": "「{x}」の下流1ホップ", "zh-hans": "“{x}”下游1跳",
        },
    },
    "q_temp_sensor": {
        "x": {"ko": "최종과열기", "en": "Final Superheater", "ja": "最終過熱器", "de": "Endüberhitzer"},
        "expect": [("10HAH10CT001", 1)],
        "frame": {
            "ko": "'{x}' 온도 센서", "en": "temperature sensor of '{x}'", "de": "Temperatursensor von „{x}“",
            "fr": "capteur de température de « {x} »", "es": "sensor de temperatura de «{x}»", "it": "sensore di temperatura di «{x}»",
            "pt": "sensor de temperatura de \"{x}\"", "nl": "temperatuursensor van '{x}'", "cs": "teplotní čidlo „{x}“",
            "ar": "حساس درجة حرارة \"{x}\"", "ja": "「{x}」の温度センサー", "zh-hans": "“{x}”的温度传感器",
        },
    },
}

# (묶음, 영역, 지금 계측 시나리오, 정답들, 데이터 언어들). 정답: (id, 허용 순위)
PLAN = [
    ("dev", "plant", None, "valves_b103_f2", [("10LAB20AA102", 2), ("10LAB20AA103", 2)]),
    ("dev", "plant", None, "fw_bypass", [("10LAB20AA103", 1)]),
    ("dev", "plant", None, "sh_temp_sensor", [("10HAH10CT001", 1)]),
    ("dev", "plant", None, "fcv_upstream", [("10LAB10AP001", 1)]),
    ("dev", "plant", None, "basement_pumps", [("10LCA10AP001", 2)]),
    ("dev", "plant", None, "pending_moc", [("MOC-2026-1021", 1)]),
    ("dev", "plant", None, "bearing_vib_proc", [("AOP-102-TB01", 1)]),
    ("dev", "plant", None, "alarm_600", [("10HAH10CT001", 3)]),
    ("dev", "plant", None, "b105_drawings", [("PID-105-HLB", 1)]),
    ("dev", "plant", None, "id_fan", [("10HLB10AN001", 1)]),
    ("dev", "plant", None, "main_steam_pressure", [("10MAA10CP001", 2)]),
    ("dev", "plant", None, "fwp_high_pressure", [("10LAB10AP001", 1)]),
    ("dev", "plant", None, "sh_abnormal", [("10HAH10BB001", 1)]),
    ("dev", "plant", None, "stuck_id", [("10LAB20AA102", 1)]),
    ("dev", "plant", "P2", "fwp_high_pressure", [("10LAB10AP001", 1)]),
    ("dev", "plant", "P2", "sh_abnormal", [("10HAH10BB001", 2)]),
    ("dev", "plant", "P2", "stuck_id", [("10LAB20AA102", 1)]),
    ("dev", "plant", "P2", "stuck_valves", [("10LAB20AA102", 1)]),
    ("dev", "plant", "P2", "low_fw_flow", [("10LAB20AA102", 2)]),
    ("dev", "plant", "P2", "hot_sensors", [("10HAH10CT001", 1)]),
    ("dev", "plant", "P2", "abnormal_b103", [("10LAB20AA102", 2)]),
    ("held", "aidc", None, "valves_b103_f2", [("DC1CDU20AA02", 2)]),
    ("held", "aidc", None, "cdu_bypass", [("DC1CDU20AA03", 1)]),
    ("held", "aidc", None, "gpu_junction", [("DC1GPU10CT01", 2)]),
    ("held", "aidc", None, "pod_a_manifold", [("DC1MNF10MV01", 1)]),
    ("held", "aidc", None, "battery_cell_temp", [("DC1BES10CT01", 1)]),
    ("held", "aidc", None, "cooling_tower_fan", [("DC1CTW10AN01", 2)]),
    ("held", "aidc", None, "thermal_runaway_proc", [("EOP-104-BAT01", 1)]),
    ("held", "aidc", None, "b105_drawings", [("MEP-105-CHL", 1)]),
    ("held", "aidc", None, "pending_mop", [("MOP-2026-1021", 1)]),
    ("held", "aidc", "D2", "stuck_valves", [("DC1CDU20AA02", 1)]),
    ("held", "aidc", "D2", "hot_gpu_racks", [("DC1GPU10RK01", 1)]),
    ("held", "aidc", "D2", "pump_high_pressure_b103", [("DC1CDU10AP01", 1)]),
    ("held", "aidc", "D2", "low_coolant_flow", [("DC1CDU20AA02", 2)]),
    ("held", "plant", None, "pp_feed_pump", [("10LAB10AP001", 3)]),
    ("held", "plant", None, "pp_smoke_fan", [("10HLB10AN001", 3)]),
    ("held", "plant", None, "pp_coal_powder", [("10HFC10AJ001", 3)]),
    ("held", "plant", None, "pp_chimney", [("10HTA10BB001", 3)]),
    ("held", "plant", None, "pp_reheat", [("10HAJ10BB001", 3)]),
    ("held", "plant", None, "pp_shaft_shake", [("10MAD10CY001", 3)]),
    ("held", "plant", None, "pp_generated", [("10MKA10CE001", 3)]),
    ("held", "plant", None, "pp_sox", [("10HTB10CQ001", 3)]),
    ("held", "aidc", None, "pp_battery_risk", [("EOP-104-BAT01", 3)]),
    ("held", "aidc", None, "pp_fire_gas", [("DC1FMS10BB01", 3)]),
    ("held", "aidc", None, "pp_optics_temp", [("DC1OPT10CT01", 3)]),
]
# 식별자만 (언어 없음)
IDS = [("plant", None, "10LAB20AA1O2", [("10LAB20AA102", 1)]), ("plant", None, "MOC-2026-1104", [("MOC-2026-1104", 1)]),
       ("aidc", None, "DC1CDUI0AP01", [("DC1CDU10AP01", 1)])]


def build() -> list:
    out = []
    for split, domain, live, key, expect in PLAN:
        # 데이터센터 원장은 한국어(+ en.json 영어 이름)만 있습니다. 발전소는 네 데이터 언어
        data_langs = DATA_LANGS if domain == "plant" else ["ko"]
        for dl in data_langs:
            for lang in LANGS:
                q = T[key][lang]
                for k, (ans, allow) in enumerate(expect):
                    out.append({"id": f"{domain}-{key}{'-' + live if live else ''}{'-' + str(k) if len(expect) > 1 else ''}-{dl}-{lang}",
                                "split": split, "template": key, "domain": domain, "live": live, "data_lang": dl, "lang": lang,
                                "query": q, "expect": ans, "allow": allow})
    for key, spec in QUOTED.items():
        for dl in DATA_LANGS:
            for lang in LANGS:
                q = spec["frame"][lang].format(x=spec["x"][dl])
                for ans, allow in spec["expect"]:
                    out.append({"id": f"{key}-{dl}-{lang}", "split": "dev", "template": key, "domain": "plant", "live": None,
                                "data_lang": dl, "lang": lang, "query": q, "expect": ans, "allow": allow})
    for domain, live, q, expect in IDS:
        for dl in (DATA_LANGS if domain == "plant" else ["ko"]):
            for ans, allow in expect:
                out.append({"id": f"id-{q}-{dl}", "split": "dev", "template": "identifier", "domain": domain, "live": live,
                            "data_lang": dl, "lang": "id", "query": q, "expect": ans, "allow": allow})
    ids = [x["id"] for x in out]
    assert len(ids) == len(set(ids)), "질의 id 가 겹칩니다"
    for key, tr in T.items():
        assert set(tr) == set(LANGS), f"{key}: 언어가 빠졌습니다 {set(LANGS) - set(tr)}"
    return out


def main() -> None:
    text = json.dumps(build(), ensure_ascii=False, indent=0) + "\n"
    if "--check" in sys.argv[1:]:
        same = OUT.exists() and OUT.read_text(encoding="utf-8") == text
        print("queries.json:", "같음" if same else "다름")
        sys.exit(0 if same else 1)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8", newline="\n")
    items = json.loads(text)
    from collections import Counter
    print(len(items), "개 질의", dict(Counter(x["split"] for x in items)))


if __name__ == "__main__":
    main()
