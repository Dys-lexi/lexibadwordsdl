import json
import requests
import threading
import time
import traceback
import random
import functools
from psycopg2.extras import Json
import itertools
from initsql import *
from flask_cors import CORS
from flask import Flask, jsonify, request, send_from_directory, send_file, Response
from waitress import serve
from concurrent.futures import ThreadPoolExecutor, as_completed,wait, FIRST_COMPLETED
print("started")
with open("tokens.txt","r") as f:
    tokens = list(map(str.strip,f))







def getnewgames():
    while True:
        with querywrapper() as query:
            query.execute("SELECT match_id FROM match WHERE parsedcorrectly = false AND (unix IS NULL OR unix < %s) ORDER BY match_id LIMIT 1000",(int(time.time())-3600*6,))
            unfinisheddownloads = query.fetchall()
            if len(unfinisheddownloads) < 10:
                query.execute("SELECT match_id FROM match ORDER BY match_id DESC LIMIT 1")
                output = query.fetchone()
                mostrecentmatchid = output and output[0] or 138067450 or 138067446
                # print(output)
                # print(mostrecentmatchid,"mostrecent")
        if len(unfinisheddownloads) < 10:
            print("doing new matches")
            r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":1000,"format":"json","order_direction":"asc","min_match_id":mostrecentmatchid,"max_unix_timestamp":int(time.time()-3600*5)})
            if "application/json" not in r.headers.get("Content-Type", ""):
                print(r.text)
                print("failed to parse match metadata")
                time.sleep(60)
                continue
            response = list(map(lambda x: x["match_id"],r.json()))
        else:
            print("redoing broken matches")
            # r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":1000,"format":"json","order_direction":"asc","match_ids":",".join(list(map(lambda x: (x[0]),unfinisheddownloads)))})
            response =list(map(lambda x: x[0],unfinisheddownloads))
        print("pulled matches")

        # yield list(map(lambda x: x["match_id"],r.json()))
        yield from map(lambda x: functools.partial(makegamebetter,x),response)

def makegamebetter(matchid):
    with querywrapper() as query:
        query.execute("SELECT parsedcorrectly FROM match WHERE match_id = %s LIMIT 1",(matchid,))
        output = query.fetchone()
        if not output or not output[0]:
            query.execute("DELETE FROM match WHERE match_id = %s",(matchid,))
            query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s)",(matchid,False,int(time.time())))
            query.commit()
    if output and output[0]: 
        return
    r = requests.get(f"https://api.deadlock-api.com/v1/matches/{matchid}/salts",params={"disable_steam":"true"})
    if r.status_code == 429 or r.status_code == 500 or r.status_code == 404 or not r.ok:
        print("429 or 500 status code for",matchid)
        return
    if "application/json" not in r.headers.get("Content-Type", ""):
        print(r.text)
        print("salt failed for",matchid)
        return 
    # print("got salt",matchid)

    if "demo_url" in r.json() and not r.json()["demo_url"]:
        # print("failed here2",r.text)
        with querywrapper() as query:
            query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s) ON CONFLICT (match_id) DO UPDATE SET parsedcorrectly = %s",(matchid,True,int(time.time()),True))
            query.commit()
        return 
    elif "demo_url" not in r.json():
        print("bad json for",matchid)
        return 
    return r.json()["demo_url"]


def parseplayers():
    while True:
        sleeptime = 30
        with querywrapper() as query:
            query.execute("SELECT account_id FROM players WHERE last_updated IS NULL or last_updated < %s ORDER BY last_updated LIMIT 1000",(int(time.time())-86400*60,))
            output = query.fetchall()
        if not output: 
            time.sleep(120)
            continue
        if len(output) > 800:
            sleeptime = 2
        try:
            print("requesting steamids")
            response = requests.get("https://api.deadlock-api.com/v1/players/steam",params={"account_ids":",".join(list(map(lambda x: str(x[0]),output)))})
        except requests.ConnectionError as error:
            time.sleep(120)
            continue
        if not response.ok:
            print("steam request broke",response.text)
            time.sleep(120)
            continue
        try:
            now = time.time()
            print("doing",len(response.json()),"steamids")
            for person in response.json():
                # print(person)
                with querywrapper() as query:
                    query.execute("UPDATE players SET personaname = %s, profileurl = %s, avatar = %s, last_updated = %s WHERE account_id = %s",(person["personaname"],person["profileurl"],person["avatar"].rsplit("/",1)[1].split(".")[0],int(time.time()),person["account_id"]) )
                    query.commit()
            print("did steamids in",f"{time.time()-now:.3f}")
        except:
            print(response.text)
            traceback.print_exc()
        time.sleep(sleeptime)



def parsegame(game,i=0):
    # print(type(game))
    if type(game) == tuple:
        print("could not parse",game)
        if game[1]:
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s) ON CONFLICT DO NOTHING",(game[0],False,int(time.time())))
                query.commit()
        return
    game = game["results"]["deadlock-chat-messages"]
    # print("saving game",game["match"]["match_id"])
    with querywrapper() as query:
        query.execute("SELECT table_name,column_name FROM information_schema.columns")
        rows = functools.reduce(lambda a,b: {**a,b[0]:[*a.get(b[0],[]),b[1]]}, query.fetchall(),{})
    # a is the accumulater b is the iterable
    with querywrapper() as query:
        query.execute("SELECT game_mode FROM match WHERE match_id = %s",(game["match"]["match_id"],))
        hasalreadybeendone = query.fetchone()
        if hasalreadybeendone and hasalreadybeendone[0]:
            query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s) ON CONFLICT (match_id) DO UPDATE SET parsedcorrectly = %s",(game["match"]["match_id"],True,True))
            query.commit()
            print("big panic! tried to overwrite a saved row")
            return
        query.execute("DELETE FROM match WHERE match_id = %s",(game["match"]["match_id"],))
        query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s)",(game["match"]["match_id"],False,int(time.time())))
        query.commit()
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        # print((*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        game["match"] = {**game["match"],"parsedcorrectly":True,"account_ids": list(map(lambda x: x["account_id"],game["match"]["players"]))}
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        query.execute(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s",(*list(map(lambda x: type(x[1]) == list and x[0] != "account_ids" and Json(x[1]) or x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        query.execute("SELECT COUNT(*) FROM chat WHERE match_id = %s",(game["match"]["match_id"],))
        output = query.fetchone()
        if not output or output[0] != len(game["chat"]):
            for chat_number, data in enumerate(game["chat"]):
                data = {**game["match"],**data,"chat_number":chat_number}
                # print(data)
                # print(rows[table])
                # print(list(filter(lambda x: x in rows[table],data)))
                # print(f"INSERT INTO {table} ({",".join(list(filter(lambda x: x in rows[table],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows[table],data))))})")
                # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows[table],data.items()))))
                query.execute(f"INSERT INTO {"chat"} ({",".join(list(filter(lambda x: x in rows["chat"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["chat"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["chat"],data.items()))))
        query.execute("SELECT COUNT(*) FROM items WHERE match_id = %s",(game["match"]["match_id"],))
        output = query.fetchone()
        if not output or output[0] != len(game.get("items",[])):
            for item_number, data in enumerate(game["items"]):
                data = {**game["match"],**data,"item_number":item_number}#,"imbuedability":Json(data["imbuedability"])}
                query.execute(f"INSERT INTO {"items"} ({",".join(list(filter(lambda x: x in rows["items"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["items"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["items"],data.items()))))
        query.execute("SELECT COUNT(*) FROM events WHERE match_id = %s",(game["match"]["match_id"],))
        output = query.fetchone()
        if not output or output[0] != len(game["events"]):
            for event_number,data in enumerate(game["events"]):
                
                # print(list(map(lambda x: x.removeprefix("attacker_") ,rows["events"])),"wqdwqdwq")
                # print("eee",data["assists"])
                for i,thing in enumerate(data["assists"] or []) :
                    data["assists"][i] = dict(filter(lambda x: x[0] in list(map(lambda x: x.removeprefix("killer_") ,rows["events"])),thing.items()))
                data = {**game["match"],**data,"event_number":event_number,"assists":Json(data["assists"])}
                # print(json.dumps(data,indent=4))
                # break
                # print(f"INSERT INTO {"events"} ({",".join(list(filter(lambda x: x in rows["events"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["events"],data))))})")
                # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows["events"],data.items()))))
                query.execute(f"INSERT INTO {"events"} ({",".join(list(filter(lambda x: x in rows["events"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["events"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["events"],data.items()))))
        for data in game["match"]["players"]:
            # print(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],data))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["players"],data)))))}")
            # print(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],data))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s",list(filter(lambda x: x in rows["players"],data)))))}, match_ids = players.match_ids || %s")
            # print((*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items()))),*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items())))))
            # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],data.items()))))
            query.execute(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s",list(filter(lambda x: x in rows["players"],data)))))}",(*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items()))),*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],data.items())))))
            # print(f"INSERT INTO playermatches ({",".join(list(filter(lambda x: x in rows["playermatches"],{**{**data,"match_id":game["match"]["match_id"]},"match_id":game["match"]["match_id"]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["playermatches"],{**data,"match_id":game["match"]["match_id"]}))))}) ON CONFLICT DO NOTHING")
            query.execute(f"INSERT INTO playermatches ({",".join(list(filter(lambda x: x in rows["playermatches"],{**data,"match_id":game["match"]["match_id"]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["playermatches"],{**data,"match_id":game["match"]["match_id"]}))))}) ON CONFLICT DO NOTHING",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["playermatches"],{**data,"match_id":game["match"]["match_id"]}.items()))))
            query.execute("INSERT INTO aliases (account_id, personaname, first_match_id, last_match_id) VALUES (%s, %s, %s, %s) ON CONFLICT (account_id, personaname) DO UPDATE SET first_match_id = LEAST(aliases.first_match_id, EXCLUDED.first_match_id), last_match_id = GREATEST(aliases.last_match_id, EXCLUDED.last_match_id)", (data["account_id"], data["personaname"], game["match"]["match_id"], game["match"]["match_id"]))
        query.commit()
    print("saved game",game["match"]["match_id"],(int(time.time()-game["match"]["unix"])//3600),"h", int(((time.time()-game["match"]["unix"]) % 3600) // 60),"m", int((time.time()-game["match"]["unix"]) % 60),"s behind")
        
        

    
def main():
    threading.Thread(target=parseplayers,daemon=True).start()
lock = threading.Lock()
app = Flask(__name__)
games = getnewgames()

@app.before_request
def before_request():
    # print("woag")
    if request.headers.get("token") not in tokens:
        return {"error": "pants"}, 401

@app.route("/submitdemo",methods=["POST"])
def submitdemo():
    for game in request.get_json():
        print("parsed game",game["results"]["deadlock-chat-messages"]["match"]["match_id"],"with",request.headers["token"])
        parsegame(game)
    return {"great":"good"}

@app.route("/geturls",methods=["POST"])
def geturls():
    print("got a request for",request.get_json()["count"],"demos from",request.headers["token"])
    toparse = []
    while len(toparse) < request.get_json()["count"]:
        with lock:
            new = itertools.islice(games,request.get_json()["count"]-len(toparse))
        toparse.extend(list(filter(bool,map(lambda x: x(),new))))
    # print(toparse)
    return toparse
CORS(app, resources={r"/*": {"origins": ["*.lexi.tf","lexi.tf"] or "*"}})
serve(app, host="0.0.0.0", port=3441, threads=20, connection_limit=200) 

if __name__ == "__main__":
    main()
