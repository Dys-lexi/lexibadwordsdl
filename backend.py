import json
import requests
import threading
import time
import traceback
import random
import functools
from psycopg2.extras import Json
from initsql import *
from concurrent.futures import ThreadPoolExecutor, as_completed,wait, FIRST_COMPLETED
from flask_cors import CORS
from cachetools import cached, LRUCache, TTLCache
from steamid_converter import Converter
from flask import Flask, jsonify, request, send_from_directory, send_file, Response
from waitress import serve
print("hello world")


@cached(cache=TTLCache(maxsize=1024, ttl=900))
def toaccountid(steamid):
    try:
        escaped = str(steamid).replace('\\', '\\\\').replace('%', '\\%').replace('_', '\\_').replace("https://","").replace("steamcommunity.com/profiles/","").removesuffix("/")
        # print("e",(escaped.endswith("/") and escaped[:-1] or escaped).replace("https://steamcommunity.com/profiles/",""))
        return int(Converter.to_steamID3((escaped.endswith("/") and escaped[:-1] or escaped).replace("https://steamcommunity.com/profiles/","")).rsplit(":",1)[1].removesuffix("]"))
    except: 
        if escaped.isdigit():
            return (toaccountid(76561197960265728+int(escaped)))
# print(Converter.to_steamID3(76561198988695461))
# print(toaccountid(1028429733+76561197960265728 ))
# print(toaccountid(1028429733 ))
@cached(cache=TTLCache(maxsize=1000, ttl=3600))
def searchforplayer(name,limit=10,offset=0,strict=False):
    limit = int(limit)
    offset = int(offset)
    # print(toaccountid(name))
    if not (accountid := toaccountid(name)) and len(name.rsplit("id/")) == 2 and not strict:
        with querywrapper() as query:
            query.execute("SELECT account_id,personaname,avatar,match_ids FROM players WHERE profileurl LIKE %s",(f"https://steamcommunity.com/id/{name.rsplit("id/",1)[1].replace("/","")}/",))
            output = query.fetchone()
            print("here1",output)
    elif accountid or strict:
        with querywrapper() as query:
            query.execute("SELECT account_id,personaname,avatar,match_ids FROM players WHERE account_id = %s",(accountid,))
            output = query.fetchone()   
            # print("HERE")
            if not output:
                output = (0,"Unknown","fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb",[])
    else: output = None
    if output:
        output = [output]
    else:

        with querywrapper() as query:
            # print("here")
            query.execute("SELECT account_id,personaname,avatar,match_ids FROM players WHERE personaname ILIKE %s ORDER BY personaname ILIKE %s DESC,cardinality(match_ids) DESC ,LENGTH(personaname) LIMIT %s OFFSET %s",(f"%{(name).replace('\\', '\\\\').replace('%', '\\%').replace('_', '\\_')}%",f"{(name).replace('\\', '\\\\').replace('%', '\\%').replace('_', '\\_')}",limit,offset))
            output = query.fetchall()
    return list(map(lambda x: {"account_id":x[0],"personaname":x[1],"avatar":x[2],"match_ids":x[3]},output))

# print(searchforplayer("[U:1:1028429733]"))
# print(json.dumps(searchforplayer("soft cl"),indent=4))


app = Flask(__name__)

@app.route("/search",methods=["POST","GET"])
def search():
    return searchforplayer(request.args.get("name") or request.get_json()["name"])

@app.route("/playerdetails",methods=["POST","GET"])
def details():
    account_id = request.args.get("account_id") or request.get_json()["account_id"]
    def yeilder():
        yield json.dumps(player := searchforplayer(account_id,strict=True))
        if not player[0]:
            return 
        yield json.dumps(getchatmessages(account_id))
    return Response(yeilder(), mimetype="application/x-ndjson")

# @app.route("/detailedinfo")

# @app.get("/search/stream")
# def search_stream():
#     name = request.args["name"]

#     def updates():
#         yield json.dumps({"type": "started"}) + "\n"
#         for player in searchforplayer(name):
#             time.sleep(1)
#             yield json.dumps({"type": "player", "player": player}) + "\n"
#     # "text/event-stream"
#     return Response(updates(), mimetype="application/x-ndjson")




CORS(app, resources={r"/*": {"origins": ["*.lexi.tf","lexi.tf"] or "*"}})
serve(app, host="0.0.0.0", port=3440, threads=40, connection_limit=200)  # prod