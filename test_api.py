import json
import requests
import threading
import time
import traceback
import random
import functools
from psycopg2.extras import Json
from initsql import *
from concurrent.futures import ThreadPoolExecutor, as_completed
print("started")
lock = threading.Lock()
lexiesiplist = {"http://haste:80":{"healthy":False,"maxrequests":0},"http://100.95.78.79:80":{"healthy":False,"maxrequests":0},"https://deadlock.allusive.me:443":{"healthy":False,"maxrequests":0}}

# Each entry represents an outstanding /parse request from this updater.
running_tasks = {server: [] for server in lexiesiplist}


def checkifworking(firstime = False):
    for server in lexiesiplist:
        try:
            healthy = requests.get(f"{server}/health", timeout=5)
        except requests.RequestException:
            healthy = False
        with lock:
            # if healthy:
                # print(server,healthy.ok, healthy.json().get("max_concurrent_requests",12))
                # print({"healthy":healthy and healthy.ok,"maxrequests":(healthy or 0) and healthy.json().get("max_concurrent_requests",12)})
                # print(healthy.json().get("max_concurrent_requests",12))
            
            lexiesiplist[server] = {"healthy":healthy and healthy.ok,"maxrequests":(healthy or 0) and (healthy.ok or 0 ) and healthy.json().get("max_concurrent_requests",12)}
    if firstime:
        print(json.dumps(lexiesiplist,indent=4))


def parse_on_backend(matchid, payload):
    attempted = set()
    while True:
        with lock:
            candidates = [server for server, healthy in lexiesiplist.items()
                          if healthy["healthy"] and server not in attempted]
            if not candidates:
                time.sleep(30)
                break
            random.shuffle(candidates)  # Spread requests evenly when counts are tied.
            server = max(candidates, key=lambda url: lexiesiplist[url]["maxrequests"] - len(running_tasks[url]))
            running_tasks[server].append(matchid)
            active = len(running_tasks[server])

        try:
            attempted.add(server)
            print("parsing match", matchid, "on", server, "active parses", active)
            response = requests.post(f"{server}/parse", json=payload, timeout=(5, None))
            # print(response.text)
            try:
                result = response.json()
            except:
                return (matchid,True)
            if response.status_code == 503 and result.get("error", {}).get("code") == "busy":
                print("backend busy", server, "match", matchid)
                with lock:
                    lexiesiplist[server]["healthy"] = False
                continue
            if "results" in result:
                return result
            print("parse failed", matchid, server, result)
            return (matchid, True)
        except requests.ConnectionError as error:
            with lock:
                lexiesiplist[server]["healthy"] = False
            print("backend unavailable", server, "match", matchid, error)
        finally:
            with lock:
                running_tasks[server].remove(matchid)
            time.sleep(1)

    print("no available backend for match", matchid)
    return (matchid, True)


def dothing(matchid):
    try:
        print("doing",matchid)
        now = time.time()
        with querywrapper() as query:
            query.execute("SELECT parsedcorrectly FROM match WHERE match_id = %s LIMIT 1",(matchid,))
            output = query.fetchone()
            if not output or not output[0]:
                query.execute("DELETE FROM match WHERE match_id = %s",(matchid,))
                query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s)",(matchid,False))
                query.commit()
        if output and output[0]: 
            print("failed here - already questioned")
            return (matchid,False)
        r = requests.get(f"https://api.deadlock-api.com/v1/matches/{matchid}/salts",params={"disable_steam":"true"})
        if r.status_code == 429 or r.status_code == 500:
            return (matchid,True)
        if "application/json" not in r.headers.get("Content-Type", ""):
            print(r.text)
            print("salt failed for",matchid)
            return (matchid, True)
        print("got salt",matchid)

        if "demo_url" in r.json() and not r.json()["demo_url"]:
            print("failed here2",r.text)
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s) ON CONFLICT (match_id) DO UPDATE SET parsedcorrectly = %s",(matchid,True,True))
                query.commit()
            return (matchid,False)
        elif "demo_url" not in r.json():
            return (matchid,True)

        url = r.json()["demo_url"]
        
        payload = {"url": url, "run": ["deadlock-chat-messages"]}
        result = parse_on_backend(matchid, payload)
        if isinstance(result, dict):
            print("Parsed demo", matchid, "in", f"{time.time()-now:.3f} seconds")
        else:
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s) ON CONFLICT (match_id) DO UPDATE SET parsedcorrectly = %s",(matchid,True,True))
                query.commit()
        return result
    except Exception:
        traceback.print_exc()
        return (matchid, True)


def dothingparttwo(matchid):
    with open("xyz.json","r") as f:
        return json.load(f)



def getnewgames():

    with querywrapper() as query:
        query.execute("SELECT match_id FROM match WHERE parsedcorrectly = false ORDER BY match_id LIMIT 1000")
        unfinisheddownloads = query.fetchall()
        if len(unfinisheddownloads) < 10:
            query.execute("SELECT match_id FROM match WHERE parsedcorrectly = true ORDER BY match_id DESC LIMIT 1")
            output = query.fetchone()
            mostrecentmatchid = output and output[0] or 138067450 or 138067446
            # print(output)
            # print(mostrecentmatchid,"mostrecent")
    if len(unfinisheddownloads) < 10:
        print("doing new matches")
        r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":1000,"format":"json","order_direction":"asc","min_match_id":mostrecentmatchid,"max_unix_timestamp":int(time.time()-3600*5)})
    else:
        print("redoing broken matches")
        r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":1000,"format":"json","order_direction":"asc","match_ids":",".join(list(map(lambda x: str(x[0]),unfinisheddownloads)))})
    print("pulled matches")
    if "application/json" not in r.headers.get("Content-Type", ""):
        print(r.text)
        print("failed to parse match metadata")
        time.sleep(60)
    yield from list(map(lambda x: x["match_id"],r.json()))

def checkifworkingstometimes():
    while True:
        time.sleep(20)
        checkifworking()
def main():
    checkifworking(firstime=True)
    time.sleep(5)
    threading.Thread(target=checkifworkingstometimes, daemon=True).start()
    while True:
        thing = lexiesiplist.copy()
        # print(list(thing.values()))
        with ThreadPoolExecutor(max_workers=(sum(list(map(lambda x:x["maxrequests"]*int(x["healthy"]),list(thing.values())))))-2) as executor:
            games = [
                executor.submit(dothing, i)
                for i in getnewgames()
            ]

            for i, game in enumerate(as_completed(games)):
                try:
                    parsegame(game.result(),i)
                except Exception:
                    traceback.print_exc()            
                
        print("sleep time!")
        time.sleep(5)

# def my_function(i):
#     print(f"Task {i} started")
#     time.sleep(10)  # Simulate blocking for 10 seconds
#     print(f"Task {i} finished")

# with ThreadPoolExecutor(max_workers=5) as executor:
#     executor.map(my_function, range(100))

def parsegame(game,i):
    # print(type(game))
    if type(game) == tuple:
        print("could not parse",game)
        if game[1]:
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s) ON CONFLICT DO NOTHING",(game[0],False))
                query.commit()
        return
    game = game["results"]["deadlock-chat-messages"]
    print("saving game",game["match"]["match_id"])
    with querywrapper() as query:
        query.execute("SELECT table_name,column_name FROM information_schema.columns")
        rows = functools.reduce(lambda a,b: {**a,b[0]:[*a.get(b[0],[]),b[1]]}, query.fetchall(),{})
    # a is the accumulater b is the iterable
    with querywrapper() as query:
        query.execute("DELETE FROM match WHERE match_id = %s",(game["match"]["match_id"],))
        query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s)",(game["match"]["match_id"],False))
        query.commit()
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        # print((*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        game["match"] = {**game["match"],"parsedcorrectly":True,"account_ids": list(map(lambda x: x["account_id"],game["match"]["players"]))}
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        query.execute(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s",(*list(map(lambda x: type(x[1]) == list and x[0] != "account_ids" and Json(x[1]) or x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        for chat_number, data in enumerate(game["chat"]):
            data = {**game["match"],**data,"chat_number":chat_number}
            # print(data)
            # print(rows[table])
            # print(list(filter(lambda x: x in rows[table],data)))
            # print(f"INSERT INTO {table} ({",".join(list(filter(lambda x: x in rows[table],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows[table],data))))})")
            # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows[table],data.items()))))
            query.execute(f"INSERT INTO {"chat"} ({",".join(list(filter(lambda x: x in rows["chat"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["chat"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["chat"],data.items()))))
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
            query.execute(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s",list(filter(lambda x: x in rows["players"],data)))))}, match_ids = players.match_ids || %s",(*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items()))),*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items())))))
        query.commit()
    print("saved game",game["match"]["match_id"])
        
        

    


if __name__ == "__main__":
    main()
